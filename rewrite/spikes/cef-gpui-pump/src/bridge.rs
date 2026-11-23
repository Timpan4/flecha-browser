//! In-process agent bridge: per-browser Chrome DevTools Protocol calls through
//! `CefBrowserHost::SendDevToolsMessage` and `AddDevToolsMessageObserver`.
//! No remote-debugging port or pipe is opened (`include/cef_browser.h`: these
//! calls "do not require an active DevTools front-end or remote-debugging session").
//!
//! Call timeouts: GPUI's Linux timers all run on one shared `Timer` thread and
//! stay there until they fire, even when dropped (`gpui/src/platform_scheduler.rs`,
//! `gpui_linux/src/linux/dispatcher.rs`). A timer per call piled up about 15,000
//! timers a second under a CDP loop, saturated that thread and stalled the CEF
//! pump behind it. So calls carry a deadline, and one sweeper task holds at most
//! one timer, set for the earliest deadline of a call in flight.

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use cef::*;
use futures::channel::oneshot;
use gpui::{AsyncApp, BackgroundExecutor, Task};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::rc::Rc as StdRc;
use std::time::{Duration, Instant};

enum Failure {
    Protocol(Value),
    Timeout,
}

struct InFlight {
    reply: oneshot::Sender<std::result::Result<Value, Failure>>,
    deadline: Instant,
}

type Pending = StdRc<RefCell<HashMap<i32, InFlight>>>;
/// Set while the sweeper waits with no call in flight; a new call wakes it.
type Wake = StdRc<RefCell<Option<oneshot::Sender<()>>>>;

struct Tab {
    id: i32,
    browser: Browser,
    _registration: Registration,
}

pub struct Bridge {
    next_id: Cell<i32>,
    pending: Pending,
    wake: Wake,
    events: StdRc<Cell<u64>>,
    timers: StdRc<Timers>,
    tabs: RefCell<Vec<Tab>>,
    timeout: Duration,
    _sweeper: Task<()>,
}

wrap_dev_tools_message_observer! {
    struct Observer {
        pending: Pending,
        events: StdRc<Cell<u64>>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            let Some(message) = message else { return 0 };
            let Ok(value) = serde_json::from_slice::<Value>(message) else { return 0 };
            match value.get("id").and_then(Value::as_i64) {
                Some(id) => {
                    let call = self.pending.borrow_mut().remove(&(id as i32));
                    if let Some(call) = call {
                        let outcome = match value.get("error") {
                            Some(err) => Err(Failure::Protocol(err.clone())),
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = call.reply.send(outcome);
                    }
                }
                None => self.events.set(self.events.get() + 1),
            }
            1
        }

        fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>) {
            let id = browser.map(|b| b.identifier());
            eprintln!("[bridge] DevTools agent detached from browser {id:?}");
        }
    }
}

/// The one place the bridge arms GPUI timers. A GPUI timer stays on the shared
/// `Timer` thread until its deadline even when dropped, so it counts as
/// outstanding until then. The sweeper never holds more than one.
pub struct Timers {
    executor: BackgroundExecutor,
    deadlines: RefCell<BinaryHeap<Reverse<Instant>>>,
    created: Cell<u64>,
    max_outstanding: Cell<usize>,
}

impl Timers {
    fn new(executor: BackgroundExecutor) -> Self {
        Self {
            executor,
            deadlines: Default::default(),
            created: Cell::new(0),
            max_outstanding: Cell::new(0),
        }
    }

    pub fn arm(&self, after: Duration) -> Task<()> {
        let now = Instant::now();
        let mut deadlines = self.deadlines.borrow_mut();
        while deadlines.peek().is_some_and(|Reverse(at)| *at <= now) {
            deadlines.pop();
        }
        deadlines.push(Reverse(now + after));
        self.max_outstanding
            .set(self.max_outstanding.get().max(deadlines.len()));
        self.created.set(self.created.get() + 1);
        self.executor.timer(after)
    }
}

/// Fails every call whose deadline passed. Sleeps until the earliest deadline
/// in flight, or until a call arrives when none is. Deadlines are the call
/// time plus one fixed timeout, so a call made while the timer is set never
/// has an earlier deadline, and the sweeper sets about one timer per timeout.
async fn sweep(pending: Pending, wake: Wake, timers: StdRc<Timers>) {
    loop {
        let earliest = pending.borrow().values().map(|c| c.deadline).min();
        let Some(deadline) = earliest else {
            let (tx, rx) = oneshot::channel();
            *wake.borrow_mut() = Some(tx);
            if rx.await.is_err() {
                return;
            }
            continue;
        };
        let now = Instant::now();
        if deadline > now {
            timers.arm(deadline - now).await;
        }
        let now = Instant::now();
        let expired: Vec<i32> = pending
            .borrow()
            .iter()
            .filter(|(_, c)| c.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            let call = pending.borrow_mut().remove(&id);
            if let Some(call) = call {
                let _ = call.reply.send(Err(Failure::Timeout));
            }
        }
    }
}

impl Bridge {
    /// `timeout` bounds each call: CEF drops malformed messages without a reply.
    pub fn new(cx: &mut AsyncApp, timeout: Duration) -> Self {
        let pending: Pending = Default::default();
        let wake: Wake = Default::default();
        let timers = StdRc::new(Timers::new(cx.background_executor().clone()));
        let sweeper = cx.spawn({
            let (pending, wake, timers) = (pending.clone(), wake.clone(), timers.clone());
            async move |_| sweep(pending, wake, timers).await
        });
        Self {
            next_id: Cell::new(1),
            pending,
            wake,
            events: Default::default(),
            timers,
            tabs: Default::default(),
            timeout,
            _sweeper: sweeper,
        }
    }

    /// GPUI timers the bridge armed, and the most that were outstanding at once.
    pub fn timers(&self) -> (u64, usize) {
        (self.timers.created.get(), self.timers.max_outstanding.get())
    }

    pub fn attach(&self, browser: &Browser) -> Result<i32> {
        let host = browser.host().context("browser has no host")?;
        let mut observer = Observer::new(self.pending.clone(), self.events.clone());
        let registration = host
            .add_dev_tools_message_observer(Some(&mut observer))
            .context("AddDevToolsMessageObserver returned null")?;
        let id = browser.identifier();
        self.tabs.borrow_mut().push(Tab {
            id,
            browser: browser.clone(),
            _registration: registration,
        });
        Ok(id)
    }

    /// Drops every observer registration and browser reference.
    pub fn detach_all(&self) {
        self.tabs.borrow_mut().clear();
        self.pending.borrow_mut().clear();
    }

    pub fn tab_ids(&self) -> Vec<i32> {
        self.tabs.borrow().iter().map(|t| t.id).collect()
    }

    pub fn events_received(&self) -> u64 {
        self.events.get()
    }

    pub async fn call(&self, tab: i32, method: &str, params: Value) -> Result<Value> {
        let host = self
            .tabs
            .borrow()
            .iter()
            .find(|t| t.id == tab)
            .and_then(|t| t.browser.host())
            .with_context(|| format!("no tab {tab}"))?;
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (reply, rx) = oneshot::channel();
        let deadline = Instant::now() + self.timeout;
        self.pending
            .borrow_mut()
            .insert(id, InFlight { reply, deadline });
        let idle_sweeper = self.wake.borrow_mut().take();
        if let Some(wake) = idle_sweeper {
            let _ = wake.send(());
        }
        let message = json!({ "id": id, "method": method, "params": params }).to_string();
        // No RefCell borrow is held across this CEF call.
        if host.send_dev_tools_message(Some(message.as_bytes())) == 0 {
            self.pending.borrow_mut().remove(&id);
            bail!("SendDevToolsMessage rejected {method}");
        }
        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(Failure::Protocol(error))) => Err(anyhow!("{method} failed: {error}")),
            Ok(Err(Failure::Timeout)) => {
                Err(anyhow!("{method}: no reply within {:?}", self.timeout))
            }
            Err(_) => Err(anyhow!("{method}: bridge dropped the call")),
        }
    }

    /// One entry per attached browser: the host's id and URL plus
    /// `Target.getTargetInfo` from that browser's own DevTools session.
    pub async fn list_tabs(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let tabs: Vec<(i32, String)> = self
            .tabs
            .borrow()
            .iter()
            .map(|t| {
                let url = t
                    .browser
                    .main_frame()
                    .map(|f| CefString::from(&f.url()).to_string())
                    .unwrap_or_default();
                (t.id, url)
            })
            .collect();
        for (id, url) in tabs {
            let info = self.call(id, "Target.getTargetInfo", json!({})).await;
            out.push(json!({
                "tab": id,
                "host_url": url,
                "target_info": info.as_ref().ok().and_then(|v| v.get("targetInfo")).cloned(),
                "error": info.err().map(|e| e.to_string()),
            }));
        }
        out
    }

    /// `Runtime.evaluate` in the page's main world, returning the value.
    pub async fn evaluate(&self, tab: i32, expression: &str) -> Result<Value> {
        let result = self
            .call(
                tab,
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            bail!("evaluate threw: {exception}");
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// `Page.captureScreenshot` as PNG bytes.
    pub async fn screenshot(&self, tab: i32) -> Result<Vec<u8>> {
        let result = self
            .call(tab, "Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .context("no data")?;
        Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
    }
}
