//! What the agent server needs from the browser. The server validates arguments, applies
//! defaults and deadlines, and formats results; a backend resolves tabs in its flecha-core
//! state and drives the engine.

use std::fmt;
use std::future::Future;

use flecha_core::TabId;
use serde::Serialize;
use serde_json::Value;

/// A tool failure the agent reads as the tool's error text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolError(pub String);

impl ToolError {
    /// The id is in no Space, window or Favorites of the regular (non-incognito) state.
    pub fn no_tab(tab: &TabId) -> Self {
        Self(format!("No tab with id {}", tab.as_str()))
    }

    /// The tab was closed while the tool waited for it.
    pub fn closed(tab: &TabId) -> Self {
        Self(format!(
            "Tab {} closed before the tool could run",
            tab.as_str()
        ))
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type ToolResult<T> = Result<T, ToolError>;

/// A row of `list_tabs`, with the field names and order of `AgentTab` in `agentTools.ts`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentTab {
    pub id: String,
    pub title: String,
    pub url: String,
    /// The Space's name; empty for Favorites.
    pub space: String,
    pub zone: Zone,
    /// Shown in some window.
    pub active: bool,
    /// In the agent's folder: a tab an agent opened.
    pub agent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    Favorites,
    Pinned,
    Today,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedTab {
    pub id: TabId,
    /// The address the engine reports once the page settles.
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Navigation<'a> {
    /// Load this address as given; the backend applies the engine's navigation rules.
    Url(&'a str),
    Back,
    Forward,
    Reload,
    /// Neither `url` nor `action`: wait for the page to settle.
    Settle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadFormat {
    /// `document.body?.innerText ?? ""`.
    Text,
    /// `document.documentElement.outerHTML`.
    Html,
    /// One `"<text> -> <href>"` line per link, the text trimmed to 80 characters.
    Links,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page {
    pub title: String,
    pub url: String,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleLevel {
    Error,
    Warning,
    Info,
    Debug,
}

impl ConsoleLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeText<'a> {
    /// Focus this element first; without it, type into whatever has focus.
    pub selector: Option<&'a str>,
    pub text: &'a str,
    /// Empty the element's value before typing.
    pub clear: bool,
    /// Press Enter afterwards.
    pub submit: bool,
}

/// What `evaluate` gives back: JavaScript `undefined`, or a JSON-serializable value.
#[derive(Clone, Debug, PartialEq)]
pub enum Evaluated {
    Undefined,
    Value(Value),
}

/// The browser, as the agent tools drive it.
///
/// Every tab-taking method resolves the id in the backend's flecha-core `BrowserState` and fails
/// with [`ToolError::no_tab`] when it is not there, so tabs outside the regular state (incognito
/// windows) are out of reach. No method but [`BrowserControl::activate_tab`] may change which
/// tab the user sees or where their keys go.
///
/// The server calls these from its own runtime thread and drops a future when the tool's
/// deadline passes or the client goes away; a backend must stop the work it stands for then.
pub trait BrowserControl: Send + Sync + 'static {
    fn list_tabs(&self) -> impl Future<Output = ToolResult<Vec<AgentTab>>> + Send;

    /// Opens `input` (command-bar text) as a background Today tab in the focused window's
    /// active Space, in the agent's folder, and waits for it to settle.
    fn open_tab(&self, input: &str) -> impl Future<Output = ToolResult<OpenedTab>> + Send;

    fn activate_tab(&self, tab: &TabId) -> impl Future<Output = ToolResult<()>> + Send;

    /// Closes a Today tab; unloads a pinned tab or favorite.
    fn close_tab(&self, tab: &TabId) -> impl Future<Output = ToolResult<()>> + Send;

    fn navigate(
        &self,
        tab: &TabId,
        to: Navigation<'_>,
    ) -> impl Future<Output = ToolResult<PageInfo>> + Send;

    fn read_page(
        &self,
        tab: &TabId,
        format: ReadFormat,
    ) -> impl Future<Output = ToolResult<Page>> + Send;

    /// The viewport as base64 PNG.
    fn screenshot(&self, tab: &TabId) -> impl Future<Output = ToolResult<String>> + Send;

    /// The tab's console messages, oldest first.
    fn console_logs(
        &self,
        tab: &TabId,
    ) -> impl Future<Output = ToolResult<Vec<ConsoleEntry>>> + Send;

    /// Clicks the first visible match; `None` when nothing visible matches. Returns the tab's
    /// address afterwards.
    fn click(
        &self,
        tab: &TabId,
        selector: &str,
    ) -> impl Future<Output = ToolResult<Option<String>>> + Send;

    /// `false` when `selector` matches nothing.
    fn type_text(
        &self,
        tab: &TabId,
        input: TypeText<'_>,
    ) -> impl Future<Output = ToolResult<bool>> + Send;

    fn press_key(&self, tab: &TabId, key: &str) -> impl Future<Output = ToolResult<()>> + Send;

    fn evaluate(
        &self,
        tab: &TabId,
        expression: &str,
    ) -> impl Future<Output = ToolResult<Evaluated>> + Send;

    /// One `wait_for` check: whether `selector` matches, or without one whether the page has
    /// finished loading. A page whose execution context went away answers `false`.
    fn probe(
        &self,
        tab: &TabId,
        selector: Option<&str>,
    ) -> impl Future<Output = ToolResult<bool>> + Send;
}
