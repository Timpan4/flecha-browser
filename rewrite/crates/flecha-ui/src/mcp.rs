//! The MCP agent bridge in the desktop app: [`flecha_services::agent`]'s loopback server, with
//! a [`BrowserControl`] backend that drives the regular windows' tabs.
//!
//! The server runs on a thread of its own. Each tool sends small requests over a channel; the
//! controller serves them on CEF's UI thread at its next tick, against the regular
//! `BrowserState` and the engine, and answers CDP calls when their replies arrive. Pauses and
//! deadlines stay on the server's Tokio runtime, so the UI arms no timer for agents.
//!
//! Agents see the regular state only. Every request resolves its tab with
//! [`agent::state::tab`] on `Model::core`, which incognito tabs are never in, so a private
//! tab's id fails as an unknown one. The tools ported from `chromium/shell/nativeHost.ts` keep
//! its scripts, key table and limits.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::Result;
use flecha_cef::{BrowserKey, DevToolsEvent, DevToolsMessage, Owner, Viewport};
use flecha_core::{Placement, TabId, WindowId};
use flecha_services::agent::{
    self, AgentServer, AgentTab, AgentToken, BrowserControl, ConsoleEntry, ConsoleLevel,
    DEFAULT_AGENT_PORT, Evaluated, Navigation, OpenedTab, Page, PageInfo, ReadFormat, ToolError,
    ToolResult, TypeText,
};
use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::Controller;
use crate::model::{self, now};

/// The bearer token's file in the profile directory. Only the owner can read it.
const TOKEN_FILE: &str = "agent-token";
/// `settle` in `nativeHost.ts`: a first pause, then a check every 100 ms for up to 8 s.
const SETTLE_FIRST: Duration = Duration::from_millis(150);
const SETTLE_POLL: Duration = Duration::from_millis(100);
const SETTLE_LIMIT: Duration = Duration::from_millis(8_000);
/// `log.length > 500` in `nativeHost.ts`.
const CONSOLE_LIMIT: usize = 500;
/// `shrink(shot.data, 1280)` in `nativeHost.ts`.
const SCREENSHOT_WIDTH: f64 = 1280.0;
/// `DEBUG_IDLE` in `nativeHost.ts`: a tab the agent used stays awake this long.
const DEBUG_IDLE: Duration = Duration::from_millis(120_000);
/// What `attach` in `nativeHost.ts` sends before the first tool runs on a page.
const SETUP: [(&str, Option<&str>); 3] = [
    (
        "Emulation.setFocusEmulationEnabled",
        Some(r#"{"enabled":true}"#),
    ),
    ("Runtime.enable", None),
    ("Log.enable", None),
];

/// Where the profile keeps the bearer token agents send.
pub(crate) fn token_path(profile: &Path) -> std::path::PathBuf {
    profile.join(TOKEN_FILE)
}

/// `agentActivity` in `nativeHost.ts`: an agent counts as connected this long after its last
/// tool call ...
const CONNECTED: Duration = Duration::from_millis(60_000);
/// ... and as busy for this long.
const BUSY: Duration = Duration::from_millis(3_500);

/// The server's side of the bridge.
#[derive(Clone)]
pub(crate) struct Bridge {
    requests: mpsc::Sender<Request>,
    activity: Arc<Mutex<Option<Activity>>>,
}

/// The last thing an agent did (`activity` in `nativeHost.ts`).
struct Activity {
    at: Instant,
    tab: Option<TabId>,
    action: String,
}

/// What the sidebar shows of a connected agent (`UiState.agent`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentStatus {
    pub(crate) busy: bool,
    pub(crate) action: String,
    pub(crate) tab: Option<TabId>,
}

pub(crate) struct Request {
    op: Op,
    reply: oneshot::Sender<ToolResult<Reply>>,
}

enum Op {
    ListTabs,
    OpenTab(String),
    Activate(TabId),
    Close(TabId),
    Status(TabId),
    Navigate(TabId, Move),
    Cdp(TabId, &'static str, Value),
    Console(TabId),
}

enum Move {
    Url(String),
    Back,
    Forward,
    Reload,
}

enum Reply {
    Tabs(Vec<AgentTab>),
    Opened(TabId),
    Done,
    Status(Status),
    Value(Value),
    Console(Vec<ConsoleEntry>),
}

struct Status {
    url: String,
    title: String,
    loading: bool,
}

fn closing() -> ToolError {
    ToolError("Flecha is closing".into())
}

fn no_page_area() -> ToolError {
    ToolError("Flecha shows no window to size the agent's tab by. Open a window first.".into())
}

fn unexpected() -> ToolError {
    ToolError("Flecha answered another request".into())
}

impl Bridge {
    async fn ask(&self, op: Op) -> ToolResult<Reply> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request { op, reply })
            .map_err(|_| closing())?;
        answer.await.map_err(|_| closing())?
    }

    async fn status(&self, tab: &TabId) -> ToolResult<Status> {
        match self.ask(Op::Status(tab.clone())).await? {
            Reply::Status(status) => Ok(status),
            _ => Err(unexpected()),
        }
    }

    async fn cdp(&self, tab: &TabId, method: &'static str, params: Value) -> ToolResult<Value> {
        match self.ask(Op::Cdp(tab.clone(), method, params)).await? {
            Reply::Value(value) => Ok(value),
            _ => Err(unexpected()),
        }
    }

    async fn done(&self, op: Op) -> ToolResult<()> {
        match self.ask(op).await? {
            Reply::Done => Ok(()),
            _ => Err(unexpected()),
        }
    }

    /// Waits for navigation the last action may have started, as `settle` does.
    async fn settle(&self, tab: &TabId) -> ToolResult<()> {
        tokio::time::sleep(SETTLE_FIRST).await;
        let deadline = tokio::time::Instant::now() + SETTLE_LIMIT;
        while tokio::time::Instant::now() < deadline {
            if !self.status(tab).await?.loading {
                return Ok(());
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
        Ok(())
    }

    /// `evaluate` in `nativeHost.ts`: the result by value, promises awaited.
    async fn run(&self, tab: &TabId, expression: &str) -> ToolResult<Evaluated> {
        let reply = self
            .cdp(
                tab,
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "timeout": agent::tools::TOOL_TIMEOUT.as_millis() as u64,
                    "returnByValue": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            let message = details
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| details.get("text").and_then(Value::as_str))
                .unwrap_or("The script threw");
            return Err(ToolError(message.to_owned()));
        }
        Ok(match reply.pointer("/result/value") {
            Some(value) => Evaluated::Value(value.clone()),
            None => Evaluated::Undefined,
        })
    }

    /// One input event pair, the release sent even when the press failed (`dispatchInput`).
    async fn input(
        &self,
        tab: &TabId,
        method: &'static str,
        down: Value,
        up: Value,
    ) -> ToolResult<()> {
        let pressed = self.cdp(tab, method, down).await;
        let released = self.cdp(tab, method, up).await;
        pressed?;
        released.map(drop)
    }
}

/// `String(value)` for what `Runtime.evaluate` returned.
fn js_string(value: Evaluated) -> String {
    match value {
        Evaluated::Undefined => "undefined".into(),
        Evaluated::Value(Value::String(text)) => text,
        Evaluated::Value(Value::Null) => "null".into(),
        Evaluated::Value(value) => value.to_string(),
    }
}

fn js_literal(text: &str) -> String {
    Value::from(text).to_string()
}

/// `centerOf` in `nativeHost.ts`.
fn center_of(selector: &str) -> String {
    format!(
        "(() => {{
    const el = document.querySelector({});
    if (!el) return undefined;
    el.scrollIntoView({{ block: 'center', inline: 'center' }});
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return undefined;
    return {{ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }};
  }})()",
        js_literal(selector)
    )
}

/// The `type` tool's focus script in `nativeHost.ts`.
fn focus(selector: &str, clear: bool) -> String {
    format!(
        "(() => {{
            const el = document.querySelector({});
            if (!el) return false;
            el.scrollIntoView({{ block: 'center' }});
            el.focus();
            if ({clear} && 'value' in el) {{ el.value = ''; el.dispatchEvent(new Event('input', {{ bubbles: true }})); }}
            return true;
          }})()",
        js_literal(selector)
    )
}

/// The key events `pressKey` in `nativeHost.ts` sends for `key`.
fn key_events(key: &str) -> (Value, Value) {
    let named = match key {
        "Enter" => Some(("Enter", 13, Some("\r"))),
        "Tab" => Some(("Tab", 9, None)),
        "Escape" => Some(("Escape", 27, None)),
        "Backspace" => Some(("Backspace", 8, None)),
        "Delete" => Some(("Delete", 46, None)),
        "ArrowLeft" => Some(("ArrowLeft", 37, None)),
        "ArrowUp" => Some(("ArrowUp", 38, None)),
        "ArrowRight" => Some(("ArrowRight", 39, None)),
        "ArrowDown" => Some(("ArrowDown", 40, None)),
        "Home" => Some(("Home", 36, None)),
        "End" => Some(("End", 35, None)),
        "PageUp" => Some(("PageUp", 33, None)),
        "PageDown" => Some(("PageDown", 34, None)),
        " " => Some(("Space", 32, Some(" "))),
        _ => None,
    };
    let mut single = key.chars();
    let (code, key_code, text) = match (named, single.next(), single.next()) {
        (Some((code, key_code, text)), _, _) => {
            (code.to_owned(), key_code, text.map(str::to_owned))
        }
        // `key.toUpperCase().charCodeAt(0)`: the first UTF-16 unit of the uppercase key.
        (None, Some(ch), None) => {
            let upper = ch.to_uppercase().collect::<String>();
            let key_code = upper.encode_utf16().next().map_or(0, u32::from);
            (format!("Key{upper}"), key_code, Some(key.to_owned()))
        }
        _ => (key.to_owned(), 0, None),
    };
    let base = json!({
        "key": key,
        "code": code,
        "windowsVirtualKeyCode": key_code,
        "nativeVirtualKeyCode": key_code,
    });
    let mut down = base.clone();
    down["type"] = json!(if text.is_some() {
        "keyDown"
    } else {
        "rawKeyDown"
    });
    if let Some(text) = text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    let mut up = base;
    up["type"] = json!("keyUp");
    (down, up)
}

impl BrowserControl for Bridge {
    fn activity(&self, tab: Option<&TabId>, action: &str) {
        *self.activity.lock().unwrap_or_else(|err| err.into_inner()) = Some(Activity {
            at: Instant::now(),
            tab: tab.cloned(),
            action: action.to_owned(),
        });
    }

    async fn list_tabs(&self) -> ToolResult<Vec<AgentTab>> {
        match self.ask(Op::ListTabs).await? {
            Reply::Tabs(tabs) => Ok(tabs),
            _ => Err(unexpected()),
        }
    }

    async fn open_tab(&self, input: &str) -> ToolResult<OpenedTab> {
        let Reply::Opened(id) = self.ask(Op::OpenTab(input.to_owned())).await? else {
            return Err(unexpected());
        };
        self.settle(&id).await?;
        let url = self.status(&id).await?.url;
        Ok(OpenedTab { id, url })
    }

    async fn activate_tab(&self, tab: &TabId) -> ToolResult<()> {
        self.done(Op::Activate(tab.clone())).await
    }

    async fn close_tab(&self, tab: &TabId) -> ToolResult<()> {
        self.done(Op::Close(tab.clone())).await
    }

    async fn navigate(&self, tab: &TabId, to: Navigation<'_>) -> ToolResult<PageInfo> {
        let step = match to {
            Navigation::Url(url) => Some(Move::Url(url.to_owned())),
            Navigation::Back => Some(Move::Back),
            Navigation::Forward => Some(Move::Forward),
            Navigation::Reload => Some(Move::Reload),
            Navigation::Settle => None,
        };
        if let Some(step) = step {
            self.done(Op::Navigate(tab.clone(), step)).await?;
        }
        self.settle(tab).await?;
        let status = self.status(tab).await?;
        Ok(PageInfo {
            url: status.url,
            title: status.title,
        })
    }

    async fn read_page(&self, tab: &TabId, format: ReadFormat) -> ToolResult<Page> {
        let code = match format {
            ReadFormat::Text => r#"document.body?.innerText ?? """#,
            ReadFormat::Html => "document.documentElement.outerHTML",
            ReadFormat::Links => {
                r#"[...document.links].map((a) => `${a.innerText.trim().slice(0, 80)} -> ${a.href}`).join("\n")"#
            }
        };
        let content = js_string(self.run(tab, code).await?);
        let status = self.status(tab).await?;
        Ok(Page {
            title: status.title,
            url: status.url,
            content,
        })
    }

    async fn screenshot(&self, tab: &TabId) -> ToolResult<String> {
        // `shrink` scaled the PNG down to 1280 pixels wide; CDP's clip scale does it here.
        let size = self
            .run(
                tab,
                "({ width: innerWidth, height: innerHeight, ratio: devicePixelRatio })",
            )
            .await?;
        let Evaluated::Value(size) = size else {
            return Err(ToolError("The page has no viewport".into()));
        };
        let number = |name| size.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        let (width, height, ratio) = (number("width"), number("height"), number("ratio"));
        let mut params = json!({ "format": "png" });
        if width > 0.0 && height > 0.0 && ratio > 0.0 && width * ratio > SCREENSHOT_WIDTH {
            params["clip"] = json!({
                "x": 0, "y": 0, "width": width, "height": height,
                "scale": SCREENSHOT_WIDTH / (width * ratio),
            });
        }
        let shot = self.cdp(tab, "Page.captureScreenshot", params).await?;
        shot.get("data")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ToolError("The screenshot came back empty".into()))
    }

    async fn console_logs(&self, tab: &TabId) -> ToolResult<Vec<ConsoleEntry>> {
        match self.ask(Op::Console(tab.clone())).await? {
            Reply::Console(entries) => Ok(entries),
            _ => Err(unexpected()),
        }
    }

    async fn click(&self, tab: &TabId, selector: &str) -> ToolResult<Option<String>> {
        let Evaluated::Value(point) = self.run(tab, &center_of(selector)).await? else {
            return Ok(None);
        };
        let (Some(x), Some(y)) = (
            point.get("x").and_then(Value::as_f64),
            point.get("y").and_then(Value::as_f64),
        ) else {
            return Ok(None);
        };
        let button =
            |kind| json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 });
        self.input(
            tab,
            "Input.dispatchMouseEvent",
            button("mousePressed"),
            button("mouseReleased"),
        )
        .await?;
        self.settle(tab).await?;
        Ok(Some(self.status(tab).await?.url))
    }

    async fn type_text(&self, tab: &TabId, input: TypeText<'_>) -> ToolResult<bool> {
        if let Some(selector) = input.selector {
            let found = self.run(tab, &focus(selector, input.clear)).await?;
            if found != Evaluated::Value(Value::Bool(true)) {
                return Ok(false);
            }
        }
        self.cdp(tab, "Input.insertText", json!({ "text": input.text }))
            .await?;
        if input.submit {
            let (down, up) = key_events("Enter");
            self.input(tab, "Input.dispatchKeyEvent", down, up).await?;
        }
        self.settle(tab).await?;
        Ok(true)
    }

    async fn press_key(&self, tab: &TabId, key: &str) -> ToolResult<()> {
        let (down, up) = key_events(key);
        self.input(tab, "Input.dispatchKeyEvent", down, up).await?;
        self.settle(tab).await
    }

    async fn evaluate(&self, tab: &TabId, expression: &str) -> ToolResult<Evaluated> {
        self.run(tab, expression).await
    }

    async fn probe(&self, tab: &TabId, selector: Option<&str>) -> ToolResult<bool> {
        let Some(selector) = selector else {
            return Ok(!self.status(tab).await?.loading);
        };
        let found = self
            .run(
                tab,
                &format!("!!document.querySelector({})", js_literal(selector)),
            )
            .await;
        match found {
            Ok(found) => Ok(found == Evaluated::Value(Value::Bool(true))),
            Err(ToolError(message))
                if message.contains("Execution context was destroyed")
                    || message.contains("Cannot find context with specified id") =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }
}

/// A CDP reply the bridge waits for.
struct Call {
    key: BrowserKey,
    kind: CallKind,
}

enum CallKind {
    /// `attach`'s setup; its reply needs no answer.
    Setup,
    Value(oneshot::Sender<ToolResult<Reply>>),
    /// Answered with the console log once the calls before it are answered, so the messages
    /// `Runtime.enable` replays are in it.
    Console(oneshot::Sender<ToolResult<Reply>>),
}

impl CallKind {
    fn canceled(&self) -> bool {
        match self {
            Self::Setup => false,
            Self::Value(reply) | Self::Console(reply) => reply.is_canceled(),
        }
    }

    fn fail(self, error: ToolError) {
        if let Self::Value(reply) | Self::Console(reply) = self {
            let _ = reply.send(Err(error));
        }
    }
}

/// A page the agent drives: its browser's generation, its console log, and its last use.
struct Attached {
    generation: u64,
    console: VecDeque<ConsoleEntry>,
    used: Instant,
}

/// The controller's side of the bridge.
pub(crate) struct Agents {
    bridge: Bridge,
    requests: mpsc::Receiver<Request>,
    server: Option<AgentServer>,
    /// Requests whose tab's browser CEF is still creating.
    waiting: Vec<Request>,
    calls: BTreeMap<i32, Call>,
    attached: BTreeMap<TabId, Attached>,
    /// The profile's token, opened once and kept across a stop and a start: a second
    /// `AgentToken` on the same file would accept only the token it last wrote.
    token: Option<Arc<AgentToken>>,
    /// Why the bridge last failed to start, for Settings (`hostError` in the shell).
    error: Option<String>,
    /// The status the windows last drew.
    shown: Option<AgentStatus>,
}

impl Default for Agents {
    fn default() -> Self {
        let (requests, receiver) = mpsc::channel();
        Self {
            bridge: Bridge {
                requests,
                activity: Arc::default(),
            },
            requests: receiver,
            server: None,
            waiting: Vec::new(),
            calls: BTreeMap::new(),
            attached: BTreeMap::new(),
            token: None,
            error: None,
            shown: None,
        }
    }
}

impl Agents {
    /// Starts the loopback server with the profile's token.
    pub(crate) fn start(&mut self, profile: &Path) -> Result<()> {
        let started = self.open_token(profile).and_then(|token| {
            Ok(AgentServer::start(
                Arc::new(self.bridge.clone()),
                token,
                DEFAULT_AGENT_PORT,
            )?)
        });
        self.error = started.as_ref().err().map(|error| format!("{error:#}"));
        self.server = Some(started?);
        Ok(())
    }

    fn open_token(&mut self, profile: &Path) -> Result<Arc<AgentToken>> {
        if let Some(token) = &self.token {
            return Ok(token.clone());
        }
        let token = Arc::new(AgentToken::open(token_path(profile))?);
        self.token = Some(token.clone());
        Ok(token)
    }

    /// The address agents connect to while the server runs.
    pub(crate) fn endpoint(&self) -> Option<String> {
        self.server.as_ref().map(AgentServer::endpoint)
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.server.as_ref().map(AgentServer::port)
    }

    /// Why the bridge couldn't start, while it is off for that reason.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The bearer token agents send, while the server runs.
    pub(crate) fn token(&self) -> Option<String> {
        self.server.as_ref()?;
        self.token.as_ref().map(|token| token.reveal())
    }

    /// Replaces the token; agents with the old one fail from their next request on.
    pub(crate) fn renew_token(&self) -> std::io::Result<()> {
        match &self.token {
            Some(token) => token.renew(),
            None => Ok(()),
        }
    }

    /// Stops the server and fails every request still open.
    pub(crate) fn stop(&mut self) {
        self.shut_down(closing);
    }

    /// Stops the server because Settings turned agents off.
    pub(crate) fn turn_off(&mut self) {
        self.shut_down(|| ToolError("Coding agents were turned off in Flecha's Settings".into()));
        self.error = None;
    }

    fn shut_down(&mut self, error: fn() -> ToolError) {
        if let Some(server) = self.server.take() {
            server.stop();
        }
        for request in self.waiting.drain(..).chain(self.requests.try_iter()) {
            let _ = request.reply.send(Err(error()));
        }
        for (_, call) in std::mem::take(&mut self.calls) {
            call.kind.fail(error());
        }
        self.attached.clear();
        *self
            .bridge
            .activity
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = None;
    }

    /// What a connected agent is doing, for the sidebar: connected for [`CONNECTED`] after its
    /// last call and busy for [`BUSY`].
    pub(crate) fn status(&self, now: Instant) -> Option<AgentStatus> {
        let activity = self
            .bridge
            .activity
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let activity = activity.as_ref()?;
        let since = now.saturating_duration_since(activity.at);
        (since < CONNECTED).then(|| AgentStatus {
            busy: since < BUSY,
            action: activity.action.clone(),
            tab: activity.tab.clone(),
        })
    }

    /// Whether the status changed since the windows last drew it, so they redraw as an agent
    /// starts, settles and goes quiet.
    pub(crate) fn status_changed(&mut self, now: Instant) -> bool {
        let status = self.status(now);
        let changed = status != self.shown;
        self.shown = status;
        changed
    }

    /// Whether an agent used the tab within `DEBUG_IDLE`, so tab sleep leaves it be.
    pub(crate) fn in_use(&self, tab: &TabId, now: Instant) -> bool {
        self.attached
            .get(tab)
            .is_some_and(|page| now.saturating_duration_since(page.used) < DEBUG_IDLE)
    }

    /// Takes a CDP reply or event meant for the bridge; returns the message when it isn't.
    pub(crate) fn devtools(&mut self, message: DevToolsMessage) -> Option<DevToolsMessage> {
        match &message.event {
            DevToolsEvent::Result {
                id,
                success,
                result,
            } => {
                let Some(call) = self.calls.remove(id) else {
                    return Some(message);
                };
                let answer = match call.kind {
                    CallKind::Setup => return None,
                    CallKind::Value(reply) => {
                        (reply, cdp_result(*success, result).map(Reply::Value))
                    }
                    CallKind::Console(reply) => {
                        let entries = self
                            .attached
                            .get(&call.key.tab)
                            .map(|page| page.console.iter().cloned().collect())
                            .unwrap_or_default();
                        (reply, Ok(Reply::Console(entries)))
                    }
                };
                let _ = answer.0.send(answer.1);
                None
            }
            DevToolsEvent::Event { method, params } => {
                let page = self.attached.get_mut(&message.tab)?;
                if let Some(entry) = console_entry(method, params) {
                    page.console.push_back(entry);
                    while page.console.len() > CONSOLE_LIMIT {
                        page.console.pop_front();
                    }
                }
                None
            }
        }
    }
}

fn cdp_result(success: bool, result: &str) -> ToolResult<Value> {
    let value: Value = serde_json::from_str(result)
        .map_err(|error| ToolError(format!("Flecha's debugger answered badly: {error}")))?;
    if success {
        return Ok(value);
    }
    Err(ToolError(
        value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The debugger command failed")
            .to_owned(),
    ))
}

/// One console line from a CDP event, as `chrome.debugger.onEvent` in `nativeHost.ts` reads it.
fn console_entry(method: &str, params: &str) -> Option<ConsoleEntry> {
    let params: Value = serde_json::from_str(params).ok()?;
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let line = |value: Option<&Value>| value.and_then(Value::as_i64).map(|line| line + 1);
    let (level, message, source) = match method {
        "Runtime.consoleAPICalled" => {
            let level = match params.get("type").and_then(Value::as_str) {
                Some("warning") => ConsoleLevel::Warning,
                Some("error" | "assert") => ConsoleLevel::Error,
                Some("debug") => ConsoleLevel::Debug,
                _ => ConsoleLevel::Info,
            };
            let message = params
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|arg| match arg.get("value") {
                    Some(Value::String(value)) => value.clone(),
                    Some(value) => value.to_string(),
                    None => text(arg.get("description"))
                        .or_else(|| text(arg.get("type")))
                        .unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            let source = params.pointer("/stackTrace/callFrames/0").map(|frame| {
                format!(
                    "{}:{}",
                    text(frame.get("url")).unwrap_or_default(),
                    line(frame.get("lineNumber")).unwrap_or(1)
                )
            });
            (level, message, source)
        }
        "Runtime.exceptionThrown" => {
            let details = params.get("exceptionDetails").cloned().unwrap_or_default();
            let message = text(details.pointer("/exception/description"))
                .or_else(|| text(details.get("text")))
                .unwrap_or_else(|| "Uncaught exception".into());
            let source = text(details.get("url"))
                .filter(|url| !url.is_empty())
                .map(|url| format!("{url}:{}", line(details.get("lineNumber")).unwrap_or(1)));
            (ConsoleLevel::Error, message, source)
        }
        "Log.entryAdded" => {
            let entry = params.get("entry").cloned().unwrap_or_default();
            let level = match entry.get("level").and_then(Value::as_str) {
                Some("verbose") => ConsoleLevel::Debug,
                Some("warning") => ConsoleLevel::Warning,
                Some("error") => ConsoleLevel::Error,
                _ => ConsoleLevel::Info,
            };
            let source =
                text(entry.get("url"))
                    .filter(|url| !url.is_empty())
                    .map(|url| match line(entry.get("lineNumber")) {
                        Some(line) => format!("{url}:{line}"),
                        None => url,
                    });
            (level, text(entry.get("text")).unwrap_or_default(), source)
        }
        _ => return None,
    };
    Some(ConsoleEntry {
        level,
        message,
        source: source.filter(|source| !source.is_empty()),
    })
}

impl Op {
    /// The tab a request drives through its page, which needs a live browser first.
    fn page(&self) -> Option<&TabId> {
        match self {
            Self::Navigate(tab, _) | Self::Cdp(tab, _, _) | Self::Console(tab) => Some(tab),
            _ => None,
        }
    }
}

impl Controller {
    /// Starts or stops the bridge as Settings → Coding Agents asks (`syncAgent` in
    /// `nativeHost.ts`). Agents reach a desktop launch only: --verify and the isolated launcher
    /// keep the profile closed to them. A bridge that can't start leaves the browser working,
    /// and Settings says why.
    pub(crate) fn sync_agents(&mut self) {
        let wanted = !self.isolated && !self.quitting && self.model.core.settings.agents_enabled();
        let running = self.agents.endpoint().is_some();
        if wanted && !running {
            match self.agents.start(&self.profile) {
                Ok(()) => eprintln!(
                    "agent bridge: {} (bearer token in {})",
                    self.agents.endpoint().unwrap_or_default(),
                    token_path(&self.profile).display()
                ),
                Err(error) => eprintln!("agent bridge: could not start: {error:#}"),
            }
        } else if !wanted && (running || self.agents.error().is_some()) {
            self.agents.turn_off();
            if running {
                eprintln!("agent bridge: stopped");
            }
        }
        if let Some(watch) = self.local_watch.get() {
            watch.exclude(self.agents.port());
        }
    }

    /// Serves the bridge's requests and fails calls whose page closed. Returns whether the
    /// windows need redrawing.
    pub(crate) fn serve_agents(&mut self) -> bool {
        let mut changed = false;
        let waiting = std::mem::take(&mut self.agents.waiting);
        let incoming = self.agents.requests.try_iter().collect::<Vec<_>>();
        for request in waiting.into_iter().chain(incoming) {
            if request.reply.is_canceled() {
                continue;
            }
            if self.quitting {
                let _ = request.reply.send(Err(closing()));
                continue;
            }
            changed |= self.serve_agent(request);
        }
        let engine = &self.engine;
        let calls = std::mem::take(&mut self.agents.calls);
        for (id, call) in calls {
            if call.kind.canceled() {
                continue;
            }
            if engine.key(&call.key.tab).as_ref() == Some(&call.key) {
                self.agents.calls.insert(id, call);
            } else {
                call.kind.fail(ToolError::closed(&call.key.tab));
            }
        }
        self.agents.attached.retain(|tab, page| {
            engine
                .key(tab)
                .is_some_and(|key| key.generation() == page.generation)
        });
        changed
    }

    fn serve_agent(&mut self, request: Request) -> bool {
        if let Some(tab) = request.op.page() {
            let tab = tab.clone();
            return match self.agent_browser(&tab) {
                Ok(Some(key)) => self.drive(key, request),
                Ok(None) => {
                    self.agents.waiting.push(request);
                    false
                }
                Err(error) => {
                    let _ = request.reply.send(Err(error));
                    false
                }
            };
        }
        let Request { op, reply } = request;
        let (result, changed) = match op {
            Op::ListTabs => (
                Ok(Reply::Tabs(agent::state::list_tabs(&self.model.core))),
                false,
            ),
            Op::OpenTab(input) => (self.agent_open(&input).map(Reply::Opened), true),
            Op::Activate(tab) => (self.agent_activate(&tab).map(|()| Reply::Done), true),
            Op::Close(tab) => (self.agent_close(&tab).map(|()| Reply::Done), true),
            Op::Status(tab) => (self.agent_status(&tab).map(Reply::Status), false),
            Op::Navigate(..) | Op::Cdp(..) | Op::Console(..) => (Err(unexpected()), false),
        };
        let _ = reply.send(result);
        changed
    }

    /// The tab, when it is in the regular state.
    fn agent_tab(&self, tab: &TabId) -> ToolResult<&flecha_core::Tab> {
        if self
            .model
            .partition(tab)
            .is_some_and(|partition| partition.is_incognito())
        {
            return Err(ToolError::no_tab(tab));
        }
        agent::state::tab(&self.model.core, tab)
    }

    fn agent_open(&mut self, input: &str) -> ToolResult<TabId> {
        if self.ready && self.agent_viewport().is_none() {
            return Err(no_page_area());
        }
        self.agent_commit(|this| {
            let id =
                agent::state::open_agent_tab(&mut this.model.core, model::tab_id(), input, now())?;
            this.agent_browser(&id)?;
            Ok((id, Vec::new()))
        })
    }

    fn agent_activate(&mut self, tab: &TabId) -> ToolResult<()> {
        self.agent_tab(tab)?;
        let window = self.model.core.focused_window().id.clone();
        self.agent_commit(|this| {
            let removed = this
                .select(&window, tab)
                .map_err(|error| ToolError(error.to_string()))?;
            Ok(((), removed))
        })
    }

    fn agent_close(&mut self, tab: &TabId) -> ToolResult<()> {
        self.agent_tab(tab)?;
        self.agent_commit(|this| {
            let mut closed = this.model.close_tab(tab);
            closed.push(tab.clone());
            Ok(((), closed))
        })
    }

    /// The one way an agent changes the regular state, so its answer, memory and disk agree.
    ///
    /// `change` edits the state and returns its value with the pages whose browsers close once
    /// the change is saved. Then the state saves:
    /// - If the snapshot never replaced the saved one, or `change` failed, the state, the front
    ///   window and the browsers go back to how they were, and the call fails.
    /// - If the snapshot is in place but its directory entry could not be synced, the change
    ///   counts as saved, as an import's does, and the regular window's status line says so.
    fn agent_commit<T>(
        &mut self,
        change: impl FnOnce(&mut Self) -> ToolResult<(T, Vec<TabId>)>,
    ) -> ToolResult<T> {
        let core = self.model.core.clone();
        let front = self.front.clone();
        let browsers = self.engine.ids().cloned().collect::<BTreeSet<_>>();
        let saved = change(self).and_then(|(value, closed)| {
            self.update_visibility();
            let synced = self
                .replace_snapshot()
                .map_err(|error| ToolError(format!("Could not save the session: {error:#}")))?;
            Ok((value, closed, synced))
        });
        match saved {
            Ok((value, closed, synced)) => {
                for id in &closed {
                    self.engine.close(id);
                }
                self.update_visibility();
                if let Err(error) = synced {
                    let warning = format!("Could not sync the session directory to disk: {error}.");
                    eprintln!("agent bridge: {warning}");
                    self.status.set(None, warning);
                }
                Ok(value)
            }
            Err(error) => {
                self.model.core = core;
                self.front = front;
                let opened = self
                    .engine
                    .ids()
                    .filter(|id| !browsers.contains(*id))
                    .cloned()
                    .collect::<Vec<_>>();
                for id in &opened {
                    self.engine.close(id);
                }
                self.update_visibility();
                Err(error)
            }
        }
    }

    fn agent_status(&self, tab: &TabId) -> ToolResult<Status> {
        let page = self.agent_tab(tab)?;
        Ok(Status {
            url: self.engine.url(tab).unwrap_or_else(|| page.url.clone()),
            title: page.title.clone(),
            loading: self.engine.is_loading(tab),
        })
    }

    /// The tab's live browser, made in the background when it has none. `None` while CEF
    /// creates it.
    fn agent_browser(&mut self, tab: &TabId) -> ToolResult<Option<BrowserKey>> {
        let url = self.agent_tab(tab)?.url.clone();
        if !self.ready {
            return Ok(None);
        }
        if self.engine.owner(tab).is_some() {
            self.engine.thaw_tab(tab);
            return Ok(self.engine.host(tab).and_then(|_| self.engine.key(tab)));
        }
        let viewport = self.agent_viewport().ok_or_else(no_page_area)?;
        let partition = self
            .model
            .partition(tab)
            .ok_or_else(|| ToolError::no_tab(tab))?;
        let owner = Owner::Tab(self.agent_window(tab));
        crate::tab_sleep::ensure_visible(&mut self.engine, tab, owner, partition, &url, viewport)
            .map_err(|error| ToolError(error.to_string()))?;
        self.update_visibility();
        Ok(None)
    }

    /// The window a background page belongs to: its own for a Today tab, else the focused one.
    fn agent_window(&self, tab: &TabId) -> WindowId {
        match self.model.core.locate(tab) {
            Some(Placement::Today { window, .. }) => window.clone(),
            _ => self.model.core.focused_window().id.clone(),
        }
    }

    /// The size of the page the user sees, for a page no window shows: the focused regular
    /// window's page, else where that window would show one.
    fn agent_viewport(&self) -> Option<Viewport> {
        // Agents belong to the regular profile: nothing incognito, not even its window's size,
        // stands in for a closed regular window.
        if self.model.regular_closed() {
            return None;
        }
        let focused = &self.model.core.focused_window().id;
        self.model
            .active(focused)
            .and_then(|tab| self.engine.viewport(tab))
            .or_else(|| self.page_areas.get(focused).copied())
            .or_else(|| {
                self.engine
                    .ids()
                    .filter(|id| {
                        matches!(self.engine.owner(id), Some(Owner::Tab(_)))
                            && self
                                .engine
                                .partition(id)
                                .is_some_and(|partition| !partition.is_incognito())
                    })
                    .find_map(|id| self.engine.viewport(id))
            })
    }

    /// Runs a page request on the tab's live browser.
    fn drive(&mut self, key: BrowserKey, request: Request) -> bool {
        let Request { op, reply } = request;
        if let Err(error) = self.attach(&key) {
            let _ = reply.send(Err(error));
            return false;
        }
        let tab = key.tab.clone();
        let (method, params, kind) = match op {
            Op::Navigate(_, step) => {
                match step {
                    Move::Url(url) => {
                        self.engine.navigate(&tab, &url);
                    }
                    Move::Back if self.engine.can_go_back(&tab) => self.engine.go_back(&tab),
                    Move::Forward if self.engine.can_go_forward(&tab) => {
                        self.engine.go_forward(&tab);
                    }
                    Move::Reload => self.engine.reload(&tab),
                    Move::Back | Move::Forward => {}
                }
                let _ = reply.send(Ok(Reply::Done));
                return false;
            }
            Op::Cdp(_, method, params) => {
                (method, Some(params.to_string()), CallKind::Value(reply))
            }
            // Any call works as the barrier; `attach` sent this one already.
            Op::Console(_) => ("Log.enable", None, CallKind::Console(reply)),
            _ => {
                let _ = reply.send(Err(unexpected()));
                return false;
            }
        };
        match self
            .engine
            .send_devtools_message(&tab, method, params.as_deref())
        {
            Ok(id) => {
                self.agents.calls.insert(id, Call { key, kind });
            }
            Err(error) => kind.fail(ToolError(error.to_string())),
        }
        false
    }

    /// `attach` in `nativeHost.ts`: the first call on a page turns on focus emulation and the
    /// console domains, and starts its log.
    fn attach(&mut self, key: &BrowserKey) -> ToolResult<()> {
        let fresh = self
            .agents
            .attached
            .get(&key.tab)
            .is_none_or(|page| page.generation != key.generation());
        if fresh {
            self.agents.attached.insert(
                key.tab.clone(),
                Attached {
                    generation: key.generation(),
                    console: VecDeque::new(),
                    used: Instant::now(),
                },
            );
            for (method, params) in SETUP {
                let id = self
                    .engine
                    .send_devtools_message(&key.tab, method, params)
                    .map_err(|error| ToolError(error.to_string()))?;
                self.agents.calls.insert(
                    id,
                    Call {
                        key: key.clone(),
                        kind: CallKind::Setup,
                    },
                );
            }
        }
        if let Some(page) = self.agents.attached.get_mut(&key.tab) {
            page.used = Instant::now();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;
    use std::sync::mpsc as std_mpsc;

    use super::*;
    use crate::Action;

    const REGULAR: &str = "https://regular.example/agent-7c1d";
    const PRIVATE: &str = "https://private.example/agent-secret-3e9a";

    fn controller(profile: &tempfile::TempDir) -> (Controller, WindowId) {
        let controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        (controller, window)
    }

    /// Runs one MCP tool call as the server does, on a Tokio thread of its own, while this
    /// thread serves the bridge as the UI's tick would. Returns the result as JSON.
    fn tool(controller: &mut Controller, name: &str, args: Value) -> Value {
        let bridge = controller.agents.bridge.clone();
        let name = name.to_owned();
        let (done, result) = std_mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            let arguments = args.as_object().cloned();
            let answer = runtime.block_on(agent::call(&bridge, &name, arguments.as_ref()));
            done.send(serde_json::to_value(answer).unwrap()).unwrap();
        });
        loop {
            controller.serve_agents();
            if let Ok(answer) = result.try_recv() {
                return answer;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn text(answer: &Value) -> &str {
        answer["content"][0]["text"].as_str().unwrap_or_default()
    }

    fn failed(answer: &Value) -> bool {
        answer["isError"] == Value::Bool(true)
    }

    fn open(controller: &mut Controller, window: &WindowId, url: &str) -> TabId {
        controller.act(window, Action::Open(url.into())).unwrap();
        controller.model.active(window).unwrap().clone()
    }

    #[test]
    fn agents_never_see_or_reach_incognito_tabs() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        let kept = open(&mut controller, &regular, REGULAR);
        controller
            .act(&regular, Action::NewIncognitoWindow)
            .unwrap();
        let private_window = controller.model.incognito_windows()[0].id.clone();
        let private = open(&mut controller, &private_window, PRIVATE);

        let listed = tool(&mut controller, "list_tabs", json!({}));
        assert!(!failed(&listed));
        let tabs: Vec<Value> = serde_json::from_str(text(&listed)).unwrap();
        assert!(
            tabs.iter()
                .any(|tab| tab["id"] == kept.as_str() && tab["url"] == REGULAR)
        );
        assert!(
            !text(&listed).contains(private.as_str()) && !text(&listed).contains("private.example"),
            "list_tabs showed an incognito tab: {listed}"
        );

        let id = private.as_str();
        for (name, args) in [
            ("activate_tab", json!({ "tabId": id })),
            ("close_tab", json!({ "tabId": id })),
            (
                "navigate",
                json!({ "tabId": id, "url": "https://example.org/" }),
            ),
            ("read_page", json!({ "tabId": id })),
            ("screenshot", json!({ "tabId": id })),
            ("console_logs", json!({ "tabId": id })),
            ("click", json!({ "tabId": id, "selector": "a" })),
            ("type", json!({ "tabId": id, "text": "secret" })),
            ("press_key", json!({ "tabId": id, "key": "Enter" })),
            (
                "evaluate",
                json!({ "tabId": id, "expression": "document.cookie" }),
            ),
            ("wait_for", json!({ "tabId": id })),
        ] {
            let answer = tool(&mut controller, name, args);
            assert!(failed(&answer), "{name} reached an incognito tab: {answer}");
            assert_eq!(text(&answer), format!("No tab with id {id}"), "{name}");
        }
        assert_eq!(controller.model.active(&private_window), Some(&private));
        assert_eq!(controller.model.tab(&private).unwrap().url, PRIVATE);
        assert_eq!(controller.model.active(&regular), Some(&kept));
    }

    #[test]
    fn open_tab_adds_a_background_agent_tab_and_close_tab_removes_it() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        let kept = open(&mut controller, &regular, REGULAR);

        let opened = tool(
            &mut controller,
            "open_tab",
            json!({ "url": "example.org/agent" }),
        );
        assert!(!failed(&opened), "{opened}");
        let opened: Value = serde_json::from_str(text(&opened)).unwrap();
        let id = TabId::new(opened["tabId"].as_str().unwrap());
        assert_eq!(opened["url"], "https://example.org/agent");
        let tab = controller.model.core.tab(&id).unwrap();
        assert!(tab.agent, "the tab is not in the agent's folder");
        assert_eq!(
            controller.model.active(&regular),
            Some(&kept),
            "open_tab moved the user's view"
        );

        let listed = tool(&mut controller, "list_tabs", json!({}));
        let tabs: Vec<Value> = serde_json::from_str(text(&listed)).unwrap();
        let row = tabs.iter().find(|tab| tab["id"] == id.as_str()).unwrap();
        assert_eq!(row["agent"], true);
        assert_eq!(row["active"], false);

        let shown = tool(
            &mut controller,
            "activate_tab",
            json!({ "tabId": id.as_str() }),
        );
        assert_eq!(text(&shown), "ok");
        assert_eq!(controller.model.active(&regular), Some(&id));

        let closed = tool(
            &mut controller,
            "close_tab",
            json!({ "tabId": id.as_str() }),
        );
        assert_eq!(text(&closed), "ok");
        assert!(controller.model.core.tab(&id).is_none());
    }

    #[test]
    fn without_a_window_to_size_by_tools_fail_instead_of_waiting() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        let kept = open(&mut controller, &regular, REGULAR);
        controller.ready = true;
        let tabs = controller.model.core.open_tabs().len();

        let answer = tool(
            &mut controller,
            "evaluate",
            json!({ "tabId": kept.as_str(), "expression": "1" }),
        );
        assert!(failed(&answer));
        assert!(text(&answer).contains("no window to size"), "{answer}");

        let opened = tool(
            &mut controller,
            "open_tab",
            json!({ "url": "example.org/agent" }),
        );
        assert!(
            failed(&opened),
            "open_tab claimed a page it can't show: {opened}"
        );
        assert!(text(&opened).contains("no window to size"), "{opened}");
        assert_eq!(controller.model.core.open_tabs().len(), tabs);
    }

    #[test]
    fn an_empty_window_sizes_the_agents_tab_by_its_page_area() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        let area = page_area();
        assert!(controller.agent_viewport().is_none());

        controller.page_area(regular.clone(), area);
        assert!(controller.agent_viewport() == Some(area));
    }

    #[test]
    fn with_only_incognito_windows_open_tab_fails_and_adds_no_tab() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        controller.page_area(regular.clone(), page_area());
        controller
            .act(&regular, Action::NewIncognitoWindow)
            .unwrap();
        let private = controller.front.clone().unwrap();
        controller.page_area(private, page_area());
        controller.act(&regular, Action::CloseWindow).unwrap();
        assert!(controller.model.regular_closed());
        controller.ready = true;
        let before = controller.model.core.clone();

        assert!(controller.agent_viewport().is_none());
        let opened = tool(
            &mut controller,
            "open_tab",
            json!({ "url": "example.org/agent" }),
        );
        assert!(failed(&opened), "{opened}");
        assert!(text(&opened).contains("no window to size"), "{opened}");
        assert!(
            controller.model.core == before,
            "open_tab changed the state"
        );
    }

    /// How the save after an agent's change fails.
    #[derive(Clone, Copy, Debug)]
    enum SaveFailure {
        /// The session directory is read-only: the snapshot never replaces the saved one.
        BeforeReplace,
        /// The snapshot replaces the saved one, then the directory entry can't be synced.
        AfterReplace,
    }

    impl SaveFailure {
        const ALL: [Self; 2] = [Self::BeforeReplace, Self::AfterReplace];

        fn arm(self, profile: &tempfile::TempDir, controller: &Controller) {
            match self {
                Self::BeforeReplace => {
                    let session = profile.path().join("session");
                    set_mode(&session, 0o555);
                    assert!(
                        tempfile::NamedTempFile::new_in(&session).is_err(),
                        "the read-only session directory still takes files (running as root?)"
                    );
                }
                Self::AfterReplace => controller.store.fail_next_directory_sync(),
            }
        }

        /// Whether the change is saved, so the agent's call succeeds.
        fn saves(self) -> bool {
            matches!(self, Self::AfterReplace)
        }

        /// Checks the agent's answer and the status line.
        fn check_answer(self, answer: &Value, controller: &Controller) {
            assert_eq!(failed(answer), !self.saves(), "{self:?}: {answer}");
            let warned = controller
                .status
                .text(None)
                .is_some_and(|text| text.contains("Could not sync the session directory"));
            assert_eq!(warned, self.saves(), "{self:?}: the sync warning");
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The controller a restart makes from what the session directory holds.
    fn reopen(profile: &tempfile::TempDir, controller: Controller) -> Controller {
        set_mode(&profile.path().join("session"), 0o755);
        drop(controller);
        Controller::open_test_profile(profile.path()).unwrap()
    }

    fn has_url(controller: &Controller, url: &str) -> bool {
        controller
            .model
            .core
            .open_tabs()
            .values()
            .any(|tab| tab.url == url)
    }

    fn archived(controller: &Controller, url: &str) -> bool {
        controller
            .model
            .core
            .archive()
            .iter()
            .any(|entry| entry.url == url)
    }

    const AGENT: &str = "https://example.org/agent";

    #[test]
    fn open_tab_answer_memory_and_disk_agree_when_its_save_fails() {
        for failure in SaveFailure::ALL {
            let profile = tempfile::tempdir().unwrap();
            let (mut controller, regular) = controller(&profile);
            controller.page_area(regular, page_area());
            failure.arm(&profile, &controller);

            let answer = tool(
                &mut controller,
                "open_tab",
                json!({ "url": "example.org/agent" }),
            );
            failure.check_answer(&answer, &controller);
            assert_eq!(has_url(&controller, AGENT), failure.saves(), "{failure:?}");
            assert!(!archived(&controller, AGENT), "{failure:?}: archived");

            let reopened = reopen(&profile, controller);
            assert_eq!(has_url(&reopened, AGENT), failure.saves(), "{failure:?}");
            assert!(!archived(&reopened, AGENT), "{failure:?}: archived");
        }
    }

    #[test]
    fn close_tab_answer_memory_and_disk_agree_when_its_save_fails() {
        for failure in SaveFailure::ALL {
            let profile = tempfile::tempdir().unwrap();
            let (mut controller, regular) = controller(&profile);
            let tab = open(&mut controller, &regular, REGULAR);
            failure.arm(&profile, &controller);

            let answer = tool(
                &mut controller,
                "close_tab",
                json!({ "tabId": tab.as_str() }),
            );
            failure.check_answer(&answer, &controller);
            let open = !failure.saves();
            assert_eq!(
                controller.model.core.tab(&tab).is_some(),
                open,
                "{failure:?}"
            );

            let reopened = reopen(&profile, controller);
            assert_eq!(reopened.model.core.tab(&tab).is_some(), open, "{failure:?}");
            assert_eq!(archived(&reopened, REGULAR), !open, "{failure:?}");
        }
    }

    #[test]
    fn activate_tab_answer_memory_and_disk_agree_when_its_save_fails() {
        for failure in SaveFailure::ALL {
            let profile = tempfile::tempdir().unwrap();
            let (mut controller, regular) = controller(&profile);
            let first = open(&mut controller, &regular, REGULAR);
            let second = open(&mut controller, &regular, "https://regular.example/second");
            assert_eq!(controller.model.active(&regular), Some(&second));
            failure.arm(&profile, &controller);

            let answer = tool(
                &mut controller,
                "activate_tab",
                json!({ "tabId": first.as_str() }),
            );
            failure.check_answer(&answer, &controller);
            let active = if failure.saves() { &first } else { &second };
            assert_eq!(
                controller.model.active(&regular),
                Some(active),
                "{failure:?}"
            );

            let reopened = reopen(&profile, controller);
            assert_eq!(reopened.model.active(&regular), Some(active), "{failure:?}");
        }
    }

    fn page_area() -> Viewport {
        Viewport::new(
            gpui::Bounds::new(
                gpui::point(gpui::px(240.), gpui::px(0.)),
                gpui::size(gpui::px(1040.), gpui::px(800.)),
            ),
            gpui::Bounds::new(
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::size(gpui::px(1280.), gpui::px(800.)),
            ),
            gpui::Bounds::new(
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::size(gpui::px(1920.), gpui::px(1080.)),
            ),
            1.0,
            None,
        )
    }

    #[test]
    fn quitting_answers_every_agent_request_with_an_error() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, regular) = controller(&profile);
        controller.act(&regular, Action::Quit).unwrap();

        let answer = tool(&mut controller, "list_tabs", json!({}));
        assert!(failed(&answer));
        assert_eq!(text(&answer), "Flecha is closing");
    }

    #[test]
    fn verify_and_isolated_launches_start_no_bridge() {
        assert!(crate::isolated_launch(true, None));
        assert!(crate::isolated_launch(false, Some("isolated".as_ref())));
        assert!(!crate::isolated_launch(false, None));
        assert!(!crate::isolated_launch(false, Some("desktop".as_ref())));

        let profile = tempfile::tempdir().unwrap();
        let (controller, _) = controller(&profile);
        assert_eq!(controller.agents.endpoint(), None);
        assert!(!token_path(profile.path()).exists());

        let verify = tempfile::tempdir().unwrap();
        let (store, loaded) =
            flecha_services::StateStore::open(&verify.path().join("session")).unwrap();
        let controller = Controller::new(
            verify.path().join("cef"),
            store,
            loaded,
            true,
            None,
            std::rc::Rc::new(std::cell::RefCell::new(None)),
        )
        .unwrap();
        assert_eq!(controller.agents.endpoint(), None);
        assert!(!token_path(verify.path()).exists());
    }

    #[test]
    fn the_settings_switch_runs_the_bridge_and_a_new_token_locks_out_the_old_one() {
        let profile = tempfile::tempdir().unwrap();
        let (mut controller, _) = controller(&profile);
        // As a desktop launch: the test profile is otherwise isolated.
        controller.isolated = false;
        controller.sync_agents();
        assert_eq!(
            controller.agents.endpoint(),
            None,
            "agents are off by default"
        );
        assert_eq!(controller.agents.token(), None);

        let enable = |controller: &mut Controller, enabled| {
            controller
                .edit_settings(|settings| {
                    settings.agent = Some(flecha_core::AgentPreference { enabled });
                    Ok(())
                })
                .unwrap();
        };
        enable(&mut controller, true);
        let port = controller
            .agents
            .port()
            .expect("the switch started the bridge");
        let old = controller.agents.token().unwrap();
        assert!(http(port, Some(&format!("Bearer {old}"))).starts_with("HTTP/1.1 200"));

        controller.agents.renew_token().unwrap();
        let new = controller.agents.token().unwrap();
        assert_ne!(new, old);
        assert!(http(port, Some(&format!("Bearer {old}"))).starts_with("HTTP/1.1 401"));
        assert!(http(port, Some(&format!("Bearer {new}"))).starts_with("HTTP/1.1 200"));

        enable(&mut controller, false);
        assert_eq!(controller.agents.endpoint(), None);
        assert_eq!(
            controller.agents.token(),
            None,
            "Settings shows no token while off"
        );
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "the switch left the server listening"
        );
    }

    fn http(port: u16, authorization: Option<&str>) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let auth =
            authorization.map_or_else(String::new, |value| format!("Authorization: {value}\r\n"));
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn the_server_listens_on_loopback_with_its_token_and_stops() {
        let profile = tempfile::tempdir().unwrap();
        let mut agents = Agents::default();
        agents.start(profile.path()).unwrap();
        let endpoint = agents.endpoint().unwrap();
        let port: u16 = endpoint
            .strip_prefix("http://127.0.0.1:")
            .and_then(|rest| rest.strip_suffix("/mcp"))
            .unwrap()
            .parse()
            .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(token_path(profile.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "others can read the token");
        }
        assert!(http(port, None).starts_with("HTTP/1.1 401"));
        let token = std::fs::read_to_string(token_path(profile.path())).unwrap();
        let listed = http(port, Some(&format!("Bearer {token}")));
        assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
        assert!(listed.contains("list_tabs"), "{listed}");

        agents.stop();
        assert_eq!(agents.endpoint(), None);
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "the server still accepts connections after stopping"
        );
    }

    #[test]
    fn console_events_read_as_the_shell_read_them() {
        let logged = console_entry(
            "Runtime.consoleAPICalled",
            r#"{"type":"warning","args":[{"type":"string","value":"slow"},{"type":"number","value":3},{"type":"object","description":"Object"}],"stackTrace":{"callFrames":[{"url":"https://a.test/app.js","lineNumber":9}]}}"#,
        )
        .unwrap();
        assert_eq!(logged.level, ConsoleLevel::Warning);
        assert_eq!(logged.message, "slow 3 Object");
        assert_eq!(logged.source.as_deref(), Some("https://a.test/app.js:10"));

        let thrown = console_entry(
            "Runtime.exceptionThrown",
            r#"{"exceptionDetails":{"text":"Uncaught","exception":{"description":"TypeError: x"},"url":"https://a.test/","lineNumber":0}}"#,
        )
        .unwrap();
        assert_eq!(thrown.level, ConsoleLevel::Error);
        assert_eq!(thrown.message, "TypeError: x");
        assert_eq!(thrown.source.as_deref(), Some("https://a.test/:1"));

        let entry = console_entry(
            "Log.entryAdded",
            r#"{"entry":{"level":"verbose","text":"net","url":""}}"#,
        )
        .unwrap();
        assert_eq!(entry.level, ConsoleLevel::Debug);
        assert_eq!(entry.source, None);
        assert!(console_entry("Page.loadEventFired", "{}").is_none());
    }

    #[test]
    fn keys_press_as_the_shell_pressed_them() {
        let (down, up) = key_events("Enter");
        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["text"], "\r");
        assert_eq!(down["windowsVirtualKeyCode"], 13);
        assert_eq!(up["type"], "keyUp");
        let (down, _) = key_events("a");
        assert_eq!(down["code"], "KeyA");
        assert_eq!(down["windowsVirtualKeyCode"], 65);
        assert_eq!(down["text"], "a");
        let (down, _) = key_events("F5");
        assert_eq!(down["type"], "rawKeyDown");
        assert_eq!(down["code"], "F5");
        assert!(down.get("text").is_none());
    }
}
