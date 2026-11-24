//! An in-memory backend for tests: real flecha-core state, and pages that are plain data.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flecha_core::{BrowserState, EpochMillis, SpaceId, TabId, WindowId};

use super::control::{
    AgentTab, BrowserControl, ConsoleEntry, Evaluated, Navigation, OpenedTab, Page, PageInfo,
    ReadFormat, ToolError, ToolResult, TypeText,
};
use super::state;

/// A page as the fake engine holds it.
#[derive(Clone, Debug, Default)]
pub struct FakePage {
    pub url: String,
    pub title: String,
    pub text: String,
    pub html: String,
    /// `(text, href)` pairs.
    pub links: Vec<(String, String)>,
    pub console: Vec<ConsoleEntry>,
    /// Selectors that match a visible element.
    pub elements: BTreeSet<String>,
    /// Whether the page has finished loading.
    pub loaded: bool,
    /// What `evaluate` returns for an expression; anything else is `undefined`.
    pub evaluations: BTreeMap<String, Evaluated>,
    /// Engine calls on this page never finish, as on a hung renderer.
    pub hung: bool,
    pub typed: Vec<String>,
    pub keys: Vec<String>,
    pub back: Vec<String>,
    /// The tab closed; calls on it fail as the shell fails them.
    pub closed: bool,
}

/// Counts of what the engine was asked to do.
#[derive(Debug, Default)]
pub struct FakeStats {
    /// `probe` calls.
    pub probes: AtomicU64,
    /// Every selector `probe` was asked about.
    pub probed: Mutex<BTreeSet<String>>,
    /// Engine calls dropped before they finished, as when a deadline passes.
    pub dropped: AtomicU64,
    /// What each tool told the sidebar it was doing, oldest first.
    pub activity: Mutex<Vec<(Option<TabId>, String)>>,
}

pub struct FakeBrowser {
    state: Mutex<BrowserState>,
    pages: Mutex<BTreeMap<TabId, FakePage>>,
    next_id: AtomicU64,
    clock: AtomicI64,
    pub stats: FakeStats,
}

impl Default for FakeBrowser {
    fn default() -> Self {
        Self::new(BrowserState::new(
            SpaceId::new("space-1"),
            WindowId::new("window-1"),
        ))
    }
}

impl FakeBrowser {
    pub fn new(state: BrowserState) -> Self {
        Self {
            state: Mutex::new(state),
            pages: Mutex::default(),
            next_id: AtomicU64::new(1),
            clock: AtomicI64::new(1_700_000_000_000),
            stats: FakeStats::default(),
        }
    }

    pub fn state(&self) -> MutexGuard<'_, BrowserState> {
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    fn pages(&self) -> MutexGuard<'_, BTreeMap<TabId, FakePage>> {
        self.pages.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Changes the tab's page, loading it first if the engine has not yet.
    pub fn page<T>(&self, tab: &TabId, change: impl FnOnce(&mut FakePage) -> T) -> T {
        let url = self
            .state()
            .tab(tab)
            .map(|tab| tab.url.clone())
            .unwrap_or_default();
        let mut pages = self.pages();
        change(pages.entry(tab.clone()).or_insert_with(|| loaded(url)))
    }

    /// A page the engine has but the regular state does not, as an incognito tab's.
    pub fn add_detached_page(&self, tab: &TabId, page: FakePage) {
        self.pages().insert(tab.clone(), page);
    }

    pub fn now(&self) -> EpochMillis {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    /// The tab's page if the tab is in the regular state.
    fn with_page<T>(&self, tab: &TabId, read: impl FnOnce(&mut FakePage) -> T) -> ToolResult<T> {
        let url = match state::tab(&self.state(), tab) {
            Ok(tab) => tab.url.clone(),
            Err(no_tab) => {
                return Err(match self.pages().get(tab) {
                    Some(page) if page.closed => ToolError::closed(tab),
                    _ => no_tab,
                });
            }
        };
        let mut pages = self.pages();
        let page = pages.entry(tab.clone()).or_insert_with(|| loaded(url));
        Ok(read(page))
    }

    /// Runs `read` on the page, or never finishes if the page is hung.
    async fn engine<T>(&self, tab: &TabId, read: impl FnOnce(&mut FakePage) -> T) -> ToolResult<T> {
        if self.with_page(tab, |page| page.hung)? {
            let _dropped = CountDrop(&self.stats.dropped);
            std::future::pending::<()>().await;
        }
        self.with_page(tab, read)
    }

    fn tear_down(&self, removed: Vec<TabId>) {
        let mut pages = self.pages();
        for id in removed {
            pages.entry(id).or_default().closed = true;
        }
    }

    fn info(&self, tab: &TabId) -> ToolResult<PageInfo> {
        self.with_page(tab, |page| PageInfo {
            url: page.url.clone(),
            title: page.title.clone(),
        })
    }
}

fn loaded(url: String) -> FakePage {
    FakePage {
        title: url.clone(),
        url,
        loaded: true,
        ..FakePage::default()
    }
}

struct CountDrop<'a>(&'a AtomicU64);

impl Drop for CountDrop<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl BrowserControl for FakeBrowser {
    fn activity(&self, tab: Option<&TabId>, action: &str) {
        self.stats
            .activity
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push((tab.cloned(), action.to_owned()));
    }

    async fn list_tabs(&self) -> ToolResult<Vec<AgentTab>> {
        Ok(state::list_tabs(&self.state()))
    }

    async fn open_tab(&self, input: &str) -> ToolResult<OpenedTab> {
        let id = TabId::new(format!(
            "agent-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        ));
        let now = self.now();
        let id = state::open_agent_tab(&mut self.state(), id, input, now)?;
        let url = self.page(&id, |page| page.url.clone());
        Ok(OpenedTab { id, url })
    }

    async fn activate_tab(&self, tab: &TabId) -> ToolResult<()> {
        let now = self.now();
        let change = state::activate(&mut self.state(), tab, now)?;
        self.tear_down(change.removed);
        Ok(())
    }

    async fn close_tab(&self, tab: &TabId) -> ToolResult<()> {
        let now = self.now();
        let change = state::close(&mut self.state(), tab, now)?;
        self.tear_down(change.removed);
        Ok(())
    }

    async fn navigate(&self, tab: &TabId, to: Navigation<'_>) -> ToolResult<PageInfo> {
        self.engine(tab, |page| match to {
            Navigation::Url(url) => {
                let previous = std::mem::replace(&mut page.url, url.to_owned());
                page.back.push(previous);
                page.title = url.to_owned();
            }
            Navigation::Back => {
                if let Some(url) = page.back.pop() {
                    page.title.clone_from(&url);
                    page.url = url;
                }
            }
            Navigation::Forward | Navigation::Reload | Navigation::Settle => {}
        })
        .await?;
        let info = self.info(tab)?;
        if let Some(tab) = self.state().tab_mut(tab) {
            tab.url.clone_from(&info.url);
            tab.title.clone_from(&info.title);
        }
        Ok(info)
    }

    async fn read_page(&self, tab: &TabId, format: ReadFormat) -> ToolResult<Page> {
        self.engine(tab, |page| Page {
            title: page.title.clone(),
            url: page.url.clone(),
            content: match format {
                ReadFormat::Text => page.text.clone(),
                ReadFormat::Html => page.html.clone(),
                ReadFormat::Links => page
                    .links
                    .iter()
                    .map(|(text, href)| {
                        format!(
                            "{} -> {href}",
                            text.trim().chars().take(80).collect::<String>()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
        })
        .await
    }

    async fn screenshot(&self, tab: &TabId) -> ToolResult<String> {
        self.engine(tab, |page| STANDARD.encode(format!("png of {}", page.url)))
            .await
    }

    async fn console_logs(&self, tab: &TabId) -> ToolResult<Vec<ConsoleEntry>> {
        self.engine(tab, |page| page.console.clone()).await
    }

    async fn click(&self, tab: &TabId, selector: &str) -> ToolResult<Option<String>> {
        self.engine(tab, |page| {
            page.elements.contains(selector).then(|| page.url.clone())
        })
        .await
    }

    async fn type_text(&self, tab: &TabId, input: TypeText<'_>) -> ToolResult<bool> {
        self.engine(tab, |page| {
            if input
                .selector
                .is_some_and(|selector| !page.elements.contains(selector))
            {
                return false;
            }
            page.typed.push(input.text.to_owned());
            if input.submit {
                page.keys.push("Enter".into());
            }
            true
        })
        .await
    }

    async fn press_key(&self, tab: &TabId, key: &str) -> ToolResult<()> {
        self.engine(tab, |page| page.keys.push(key.to_owned()))
            .await
    }

    async fn evaluate(&self, tab: &TabId, expression: &str) -> ToolResult<Evaluated> {
        self.engine(tab, |page| {
            page.evaluations
                .get(expression)
                .cloned()
                .unwrap_or(Evaluated::Undefined)
        })
        .await
    }

    async fn probe(&self, tab: &TabId, selector: Option<&str>) -> ToolResult<bool> {
        self.stats.probes.fetch_add(1, Ordering::Relaxed);
        if let Some(selector) = selector {
            self.stats
                .probed
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .insert(selector.to_owned());
        }
        self.engine(tab, |page| match selector {
            Some(selector) => page.elements.contains(selector),
            None => page.loaded,
        })
        .await
    }
}
