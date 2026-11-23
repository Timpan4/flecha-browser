//! The agent tools' reads and writes of flecha-core state. A backend runs these on the
//! `BrowserState` it owns, so agent tabs are placed and owned by the same rules as the sidebar's.

use std::collections::BTreeSet;

use flecha_core::suggestions::{InputResolution, resolve_input};
use flecha_core::{BrowserState, CloseTab, EpochMillis, Tab, TabChange, TabId, WindowId};

use super::control::{AgentTab, ToolError, ToolResult, Zone};

const BLANK: &str = "about:blank";
const BLANK_TITLE: &str = "New Tab";

/// Every open tab: Favorites, then each Space's pinned tabs followed by its Today tabs in every
/// window, as `listTabs` in `chromium/shell/nativeHost.ts` orders them.
pub fn list_tabs(state: &BrowserState) -> Vec<AgentTab> {
    let visible: BTreeSet<&TabId> = state
        .windows()
        .iter()
        .flat_map(|window| window.visible_tabs())
        .collect();
    let mut tabs = Vec::new();
    let mut add = |id: &TabId, space: &str, zone: Zone| {
        if let Some(tab) = state.tab(id) {
            tabs.push(AgentTab {
                id: id.as_str().to_owned(),
                title: tab
                    .custom_title
                    .as_deref()
                    .filter(|title| !title.is_empty())
                    .unwrap_or(&tab.title)
                    .to_owned(),
                url: tab.url.clone(),
                space: space.to_owned(),
                zone,
                active: visible.contains(id),
                agent: tab.agent,
            });
        }
    };
    for id in state.favorites() {
        add(id, "", Zone::Favorites);
    }
    for space in state.spaces() {
        for id in space.pinned_tabs() {
            add(id, &space.name, Zone::Pinned);
        }
        for window in state.windows() {
            for id in window
                .spaces
                .get(&space.id)
                .into_iter()
                .flat_map(|ws| &ws.today)
            {
                add(id, &space.name, Zone::Today);
            }
        }
    }
    tabs
}

/// Opens command-bar `input` as an agent tab at the top of the focused window's Today list in
/// its active Space, without showing it. Text that resolves to nothing opens a blank tab, as
/// `addTab` in `chromium/shell/background.ts` does.
pub fn open_agent_tab(
    state: &mut BrowserState,
    id: TabId,
    input: &str,
    now: EpochMillis,
) -> ToolResult<TabId> {
    let url = match resolve_input(input, state.settings.search_engine) {
        InputResolution::Navigate(url) | InputResolution::Search(url) => url,
        InputResolution::Invalid => BLANK.to_owned(),
    };
    let mut tab = Tab::new(url, now);
    if tab.url == BLANK {
        tab.title = BLANK_TITLE.to_owned();
    }
    tab.agent = true;
    let window = state.focused_window().id.clone();
    if state.open_today(id.clone(), tab, &window, None, false) {
        Ok(id)
    } else {
        Err(ToolError(format!("Tab {} couldn’t be opened", id.as_str())))
    }
}

/// The tab, if it is in the state.
pub fn tab<'a>(state: &'a BrowserState, id: &TabId) -> ToolResult<&'a Tab> {
    match state.locate(id) {
        Some(_) => state.tab(id).ok_or_else(|| ToolError::no_tab(id)),
        None => Err(ToolError::no_tab(id)),
    }
}

/// Shows the tab in the focused window (its own window for a Today tab) and marks it used.
/// Claiming a shared tab can end splits elsewhere; the caller tears down every tab in `removed`.
pub fn activate(
    state: &mut BrowserState,
    id: &TabId,
    now: EpochMillis,
) -> ToolResult<TabChange<Option<WindowId>>> {
    tab(state, id)?;
    let window = state.focused_window().id.clone();
    Ok(state.activate(id, &window, Some(now)))
}

/// Closes a Today tab and unloads a pinned tab or favorite. The caller tears down the engine
/// views of every tab in `removed`.
pub fn close(
    state: &mut BrowserState,
    id: &TabId,
    now: EpochMillis,
) -> ToolResult<TabChange<CloseTab>> {
    tab(state, id)?;
    Ok(state.close_tab(id, now))
}
