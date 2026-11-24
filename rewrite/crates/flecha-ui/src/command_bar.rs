use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_io::Timer;

use flecha_core::{
    ArchivedTab, BrowserState, SearchEngine, Settings, Tab, TabId, WindowId,
    input_text::trim_input,
    shortcuts,
    suggestions::{
        self, InlineCompletion, InputResolution, MatchQuality, SuggestionAction, VisitedPage,
    },
};
use flecha_history::{HistoryStore, PAGE};
use gpui::{
    AppContext as _, ClickEvent, ClipboardItem, Context, Div, FontWeight, InteractiveElement as _,
    IntoElement as _, ParentElement as _, ScrollHandle, StatefulInteractiveElement as _,
    Styled as _, Task, Window, div, prelude::FluentBuilder as _, px,
};

use crate::icons::icon;
use crate::settings_view::{RESTORE_BEFORE_IMPORT, SettingsTab};
use crate::shortcuts::{Navigation, Shortcut, ShownWindow, platform};
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, kbd, overlay_panel};
use crate::{Action, Controller, now};

mod remote;
#[cfg(test)]
mod remote_tests;
pub(crate) use remote::SuggestionClient;
use remote::{RemoteSuggestions, SuggestionRequest};

// Scores and per-source row limits from chromium/shell/{lib/commandBar.ts,ui/app.ts}.
const COMPLETION_SCORE: u16 = 1500;
const ADDRESS_SCORE: u16 = 1450;
const SEARCH_SCORE: u16 = 1200;
const REMOTE_SCORE: u16 = 1020;
const TAB_ROWS: usize = 6;
const HISTORY_ROWS: usize = 6;
const ARCHIVE_ROWS: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    NewTab,
    EditLocation,
    Split,
    Peek,
    /// "New Flechinha window" (`ui/app.ts` `windowMenu`): the address opens in a new Flechinha.
    Flechinha,
    OpenSettings,
}

impl Command {
    fn label(self) -> &'static str {
        match self {
            Self::NewTab => "New tab",
            Self::EditLocation => "Edit address",
            Self::Split => "Split with a new tab",
            Self::Peek => "Open Peek",
            Self::Flechinha => "New Flechinha window",
            Self::OpenSettings => "Settings",
        }
    }

    /// Shipping's three bar modes (`paletteMode`) offer actions; Peek and Flechinha pick a page.
    fn offers_actions(self) -> bool {
        matches!(self, Self::NewTab | Self::EditLocation | Self::Split)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Target {
    Tab(TabId, String),
    Address(String),
    Search(String),
    Copy(String),
    /// A Flecha command (`ACTIONS`).
    Action(Shortcut),
    /// An archived tab's address, which opens as a new page.
    Archived(String),
}

struct Suggestion {
    target: Target,
    label: String,
    detail: String,
    kind: &'static str,
    score: u16,
    recency: i64,
}

impl Suggestion {
    fn page_url(&self) -> Option<&str> {
        match &self.target {
            Target::Tab(_, url) => Some(url),
            Target::Address(url) if matches!(self.kind, "History" | "Completion") => Some(url),
            Target::Archived(url) => Some(url),
            _ => None,
        }
    }
}

#[derive(Default)]
struct CommandMatches {
    rows: Vec<Suggestion>,
    selected: usize,
    selection_moved: bool,
    completion: Option<InlineCompletion>,
    request: Option<Arc<SuggestionRequest>>,
    engine: SearchEngine,
    suggestions_enabled: bool,
}

pub(crate) struct CommandBar {
    mode: Command,
    /// Tab in an empty bar searches only the actions (`actionMode`).
    actions: bool,
    input: TextField,
    typed: String,
    matches: CommandMatches,
    request_task: Option<Task<()>>,
    scroll: ScrollHandle,
}

#[derive(Debug, Eq, PartialEq)]
enum Key {
    Up,
    Down,
    Submit,
    Dismiss,
    Tab,
    Edit,
}

fn command_key(stroke: &gpui::Keystroke) -> Key {
    if stroke.modifiers != gpui::Modifiers::none() {
        return Key::Edit;
    }
    match stroke.key.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "enter" => Key::Submit,
        "escape" => Key::Dismiss,
        "tab" => Key::Tab,
        _ => Key::Edit,
    }
}

fn selection_after(index: usize, len: usize, key: Key) -> usize {
    if len == 0 {
        return 0;
    }
    match key {
        Key::Up if index == 0 => len - 1,
        Key::Up => index - 1,
        Key::Down => (index + 1) % len,
        _ => index,
    }
}

fn location<'a>(
    core: &'a BrowserState,
    window: &WindowId,
    mode: Command,
) -> Option<Option<&'a Tab>> {
    let window = core.window(window)?;
    let tab = window
        .spaces
        .get(&window.active_space_id)
        .and_then(|space| space.active_tab_id.as_ref())
        .and_then(|tab| core.tab(tab));
    Some(tab.filter(|_| mode == Command::EditLocation))
}

fn suggestions_for(
    sources: Sources<'_>,
    current: Option<&TabId>,
    text: &str,
    may_complete: bool,
) -> (Vec<Suggestion>, Option<InlineCompletion>) {
    let Sources {
        tabs: core,
        settings,
        history,
        archive,
        ..
    } = sources;
    let mut rows = Vec::new();
    let private = suggestions::contains_private_jwt(text);
    let current_url = current
        .and_then(|id| core.tab(id))
        .map(|tab| tab.url.as_str());
    for row in suggestions::developer_suggestions(text, current_url, &settings.environments, now())
    {
        let target = match row.action {
            SuggestionAction::Copy(text) => Target::Copy(text),
            SuggestionAction::Open(url) if !private => Target::Address(url),
            SuggestionAction::Open(_) => continue,
        };
        rows.push(Suggestion {
            target,
            label: row.label,
            detail: row.detail,
            kind: "Local",
            score: ADDRESS_SCORE,
            recency: 0,
        });
    }
    if private {
        return (rows, None);
    }
    let resolved = suggestions::resolve_input(text, settings.search_engine);
    match &resolved {
        InputResolution::Navigate(url) => rows.push(Suggestion {
            target: Target::Address(url.clone()),
            label: suggestions::display_url(url),
            detail: String::new(),
            kind: "Go to",
            score: ADDRESS_SCORE,
            recency: 0,
        }),
        InputResolution::Search(url) => rows.push(Suggestion {
            target: Target::Search(url.clone()),
            label: trim_input(text).into(),
            detail: settings.search_engine.name().into(),
            kind: "Search",
            score: SEARCH_SCORE,
            recency: 0,
        }),
        InputResolution::Invalid => {}
    }

    let mut tabs = Vec::new();
    let empty = trim_input(text).is_empty();
    let favorites = core
        .favorites()
        .iter()
        .map(|id| (id, None, ("Favorite", 3)));
    let pinned = core.spaces().iter().flat_map(|space| {
        space
            .pinned_tabs()
            .into_iter()
            .map(move |id| (id, Some(space), ("Pinned", 2)))
    });
    let today = core
        .windows()
        .iter()
        .flat_map(|window| &window.spaces)
        .flat_map(|(space, view)| {
            let space = core.space(space);
            view.today
                .iter()
                .map(move |id| (id, space, ("Open tab", 1)))
        });
    for (id, space, (kind, rank)) in favorites.chain(pinned).chain(today) {
        if Some(id) == current && !empty {
            continue;
        }
        let Some(tab) = core.tab(id) else { continue };
        let title = tab.custom_title.as_ref().unwrap_or(&tab.title);
        let address = suggestions::display_url(&tab.url);
        let quality = if empty {
            Some(MatchQuality::Contains)
        } else {
            suggestions::match_quality(text, &[title, &address])
        };
        let Some(quality) = quality else { continue };
        let detail = match space {
            Some(space) => format!("{} · {address}", space.name),
            None => address,
        };
        tabs.push(Suggestion {
            target: Target::Tab(id.clone(), tab.url.clone()),
            label: title.clone(),
            detail,
            kind,
            score: if empty {
                rank
            } else {
                match quality {
                    MatchQuality::Start => 1300,
                    MatchQuality::Word => 1150,
                    MatchQuality::Contains => 950,
                }
            },
            recency: tab.last_active.unwrap_or_default() as i64,
        });
    }
    tabs.sort_by_key(|row| Reverse((row.score, row.recency)));
    if !empty {
        tabs.truncate(TAB_ROWS);
    }
    rows.extend(tabs);

    let history = history
        .filter(|_| !empty)
        .and_then(|store| store.suggest(trim_input(text), PAGE).ok())
        .unwrap_or_default();
    rows.extend(
        history
            .iter()
            .filter_map(|entry| {
                let address = suggestions::display_url(&entry.url);
                let quality = suggestions::match_quality(text, &[&entry.title, &address])?;
                Some(Suggestion {
                    target: Target::Address(entry.url.clone()),
                    label: entry.title.clone(),
                    detail: address,
                    kind: "History",
                    score: match quality {
                        MatchQuality::Start => 1150,
                        MatchQuality::Word => 1050,
                        MatchQuality::Contains => 900,
                    },
                    recency: entry.last_visit_ms,
                })
            })
            .take(HISTORY_ROWS),
    );
    // Archived tabs, newest first, reopen as a new page (`reopen`, chromium/shell/ui/app.ts).
    rows.extend(
        archive
            .iter()
            .filter(|_| !empty)
            .filter_map(|entry| {
                let address = suggestions::display_url(&entry.url);
                let quality = suggestions::match_quality(text, &[&entry.title, &address])?;
                let titled = !entry.title.is_empty();
                Some(Suggestion {
                    target: Target::Archived(entry.url.clone()),
                    label: if titled {
                        entry.title.clone()
                    } else {
                        address.clone()
                    },
                    detail: if titled { address } else { String::new() },
                    kind: "Archived",
                    score: match quality {
                        MatchQuality::Start => 1100,
                        MatchQuality::Word => 1000,
                        MatchQuality::Contains => 850,
                    },
                    recency: entry.closed_at,
                })
            })
            .take(ARCHIVE_ROWS),
    );

    let completion = may_complete
        .then(|| {
            let tabs = core.open_tabs();
            let candidates = tabs.values().map(|tab| VisitedPage {
                url: &tab.url,
                title: tab.custom_title.as_deref().unwrap_or(&tab.title),
                weight: tab.last_active.unwrap_or_default() as u64,
            });
            let visited = history.iter().map(|entry| VisitedPage {
                url: &entry.url,
                title: &entry.title,
                weight: entry.visit_count,
            });
            suggestions::inline_completion(text, candidates.chain(visited)).inspect(|completion| {
                let target = tabs
                    .into_iter()
                    .find(|&(id, tab)| Some(id) != current && tab.url == completion.url)
                    .map_or_else(
                        || Target::Address(completion.url.clone()),
                        |(id, _)| Target::Tab(id.clone(), completion.url.clone()),
                    );
                rows.push(Suggestion {
                    target,
                    label: completion
                        .title
                        .clone()
                        .unwrap_or_else(|| completion.text.clone()),
                    detail: completion
                        .title
                        .as_ref()
                        .map_or_else(String::new, |_| completion.text.clone()),
                    kind: "Completion",
                    score: COMPLETION_SCORE,
                    recency: 0,
                });
            })
        })
        .flatten();

    rows.sort_by_key(|row| Reverse((row.score, row.recency)));
    if !empty {
        let mut seen = BTreeSet::new();
        rows.retain(|row| row.page_url().is_none_or(|url| seen.insert(url.to_owned())));
    }
    (rows, completion)
}

/// What a window's command bar draws on. The tabs are the window's own: an incognito window's
/// are its session's. Preferences and history are the regular profile's, which an incognito
/// window reads and never writes (`incognito.ts`, `background.ts` `searchHistory`).
#[derive(Clone, Copy)]
pub(crate) struct Sources<'a> {
    tabs: &'a BrowserState,
    settings: &'a Settings,
    history: Option<&'a HistoryStore>,
    /// The regular profile's archive; an incognito window keeps none.
    archive: &'a [ArchivedTab],
    remote: RemoteSuggestions,
}

impl<'a> Sources<'a> {
    #[cfg(test)]
    fn regular(core: &'a BrowserState, history: Option<&'a HistoryStore>) -> Self {
        Self {
            tabs: core,
            settings: &core.settings,
            history,
            archive: core.archive(),
            remote: RemoteSuggestions::Allowed,
        }
    }
}

impl Controller {
    pub(crate) fn command_sources(&self, window: &WindowId) -> Option<Sources<'_>> {
        let incognito = self.model.is_incognito(window);
        Some(Sources {
            tabs: self.model.state(window)?,
            settings: &self.model.core.settings,
            history: self.history.store(),
            archive: if incognito {
                &[]
            } else {
                self.model.core.archive()
            },
            remote: if incognito {
                RemoteSuggestions::Refused
            } else {
                RemoteSuggestions::Allowed
            },
        })
    }
}

impl CommandMatches {
    /// What the bar shows for `text`: only the actions in action mode, and otherwise the pages
    /// with the actions ranked among them when `mode` offers actions.
    fn for_window(
        state: &Controller,
        sources: Sources<'_>,
        window: &WindowId,
        text: &str,
        may_complete: bool,
        mode: Command,
        actions: bool,
    ) -> Self {
        if actions {
            return Self {
                rows: action_rows(state, window, text, true),
                engine: sources.settings.search_engine,
                suggestions_enabled: sources.settings.search_suggestions_enabled(),
                ..Self::default()
            };
        }
        let mut matches = Self::new(sources, state.model.active(window), text, may_complete);
        if mode.offers_actions() {
            matches.rows.extend(action_rows(state, window, text, false));
            matches
                .rows
                .sort_by_key(|row| Reverse((row.score, row.recency)));
        }
        matches
    }

    fn new(sources: Sources<'_>, current: Option<&TabId>, text: &str, may_complete: bool) -> Self {
        let (rows, completion) = suggestions_for(sources, current, text, may_complete);
        Self {
            rows,
            completion,
            request: SuggestionRequest::new(sources.settings, sources.remote, text).map(Arc::new),
            engine: sources.settings.search_engine,
            suggestions_enabled: sources.settings.search_suggestions_enabled(),
            ..Self::default()
        }
    }

    fn select(&mut self, key: Key) {
        self.selected = selection_after(self.selected, self.rows.len(), key);
        self.selection_moved = true;
    }

    fn expects(&self, request: &Arc<SuggestionRequest>) -> bool {
        self.request
            .as_ref()
            .is_some_and(|latest| Arc::ptr_eq(latest, request))
    }

    /// Shows `previous`'s search suggestions until this query's own answer replaces them, so the
    /// list keeps its height while typing. Only a query that will be answered keeps them: one
    /// that asks nothing (empty, an address, private, suggestions off) or asks another engine
    /// shows its local rows alone.
    fn keep_remote(&mut self, previous: &CommandMatches) {
        if self.request.is_none() || previous.engine != self.engine {
            return;
        }
        let kept = previous
            .remote()
            .filter(|row| !self.rows.iter().any(|local| local.target == row.target))
            .map(|row| remote_row(row.label.clone(), row.target.clone()))
            .collect::<Vec<_>>();
        let insert = self.rows.partition_point(|row| row.score > REMOTE_SCORE);
        self.rows.splice(insert..insert, kept);
    }

    fn remote(&self) -> impl Iterator<Item = &Suggestion> {
        self.rows.iter().filter(|row| row.kind == REMOTE_KIND)
    }

    /// Replaces the suggestions shown so far with `request`'s answer, if it is still the query's.
    /// A selection the arrows moved stays on its row, or at its place when that row was replaced.
    fn complete(&mut self, request: &Arc<SuggestionRequest>, terms: Vec<String>) -> bool {
        if !self.expects(request) {
            return false;
        }
        self.request = None;
        let selected = self
            .selection_moved
            .then(|| self.rows.get(self.selected).map(|row| row.target.clone()))
            .flatten();
        self.rows.retain(|row| row.kind != REMOTE_KIND);
        let insert = self.rows.partition_point(|row| row.score > REMOTE_SCORE);
        let engine = request.engine;
        self.rows.splice(
            insert..insert,
            terms.into_iter().map(|term| {
                let target = Target::Search(engine.search_url(&term));
                remote_row(term, target)
            }),
        );
        if let Some(target) = selected {
            self.selected = self
                .rows
                .iter()
                .position(|row| row.target == target)
                .unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
        }
        true
    }
}

const REMOTE_KIND: &str = "Search suggestion";

fn remote_row(label: String, target: Target) -> Suggestion {
    Suggestion {
        target,
        label,
        detail: String::new(),
        kind: REMOTE_KIND,
        score: REMOTE_SCORE,
        recency: 0,
    }
}

/// A command the bar runs, found by any word of its label or keywords (`ACTIONS`,
/// chromium/shell/ui/app.ts:1165-1206). `empty_bar` ones fill an empty bar after its tabs.
pub(crate) struct BarAction {
    pub(crate) label: &'static str,
    keywords: &'static str,
    icon: &'static str,
    /// The id of its keyboard shortcut, shown beside it.
    pub(crate) shortcut: Option<&'static str>,
    empty_bar: bool,
    pub(crate) run: Shortcut,
}

const fn bar_action(
    label: &'static str,
    keywords: &'static str,
    icon: &'static str,
    shortcut: Option<&'static str>,
    run: Shortcut,
) -> BarAction {
    BarAction {
        label,
        keywords,
        icon,
        shortcut,
        empty_bar: false,
        run,
    }
}

/// Shipping's actions in its order, then the tab and window commands its shortcuts run.
pub(crate) const ACTIONS: &[BarAction] = &[
    BarAction {
        empty_bar: true,
        ..bar_action(
            "New Space",
            "space create add",
            "space",
            Some("newSpace"),
            Shortcut::NewSpace,
        )
    },
    bar_action(
        "Edit Space",
        "space rename emoji colour color icon",
        "space",
        None,
        Shortcut::EditSpace,
    ),
    BarAction {
        empty_bar: true,
        ..bar_action(
            "New Flechinha Window",
            "flechinha little mini small window quick lookup",
            "miniWindow",
            Some("newFlechinha"),
            Shortcut::Command(Command::Flechinha),
        )
    },
    bar_action(
        "Settings",
        "preferences settings config options",
        "settings",
        Some("settings"),
        Shortcut::Command(Command::OpenSettings),
    ),
    bar_action(
        "Toggle Sidebar",
        "sidebar hide show",
        "sidebar",
        Some("toggleSidebar"),
        Shortcut::Sidebar,
    ),
    bar_action(
        "Clear Today Tabs",
        "clear archive today close",
        "archive",
        Some("clearToday"),
        Shortcut::ClearToday,
    ),
    bar_action(
        "Show Downloads",
        "downloads files",
        "download",
        None,
        Shortcut::Downloads,
    ),
    bar_action(
        "Show History",
        "history visited pages clear",
        "history",
        Some("history"),
        Shortcut::History,
    ),
    bar_action(
        "Import from another browser",
        "import migrate bookmarks history arc chrome safari firefox brave edge vivaldi opera",
        "import",
        None,
        Shortcut::SettingsTab(SettingsTab::Import),
    ),
    bar_action(
        RESTORE_BEFORE_IMPORT,
        "undo restore revert backup import flecha electron state",
        "import",
        None,
        Shortcut::RestoreBeforeImport,
    ),
    bar_action(
        "Show Boosts",
        "boosts scripts css styles",
        "zap",
        Some("editBoost"),
        Shortcut::SettingsTab(SettingsTab::Boosts),
    ),
    bar_action(
        "Show Archive",
        "archive archived closed tabs reopen",
        "archive",
        None,
        Shortcut::Archive,
    ),
    bar_action(
        "New Window",
        "window new browser",
        "tab",
        None,
        Shortcut::NewWindow,
    ),
    bar_action(
        "New Incognito Window",
        "incognito private window",
        "tab",
        None,
        Shortcut::NewIncognitoWindow,
    ),
    bar_action(
        "Copy Link",
        "copy link url address share",
        "link",
        Some("copyLink"),
        Shortcut::CopyLink(false),
    ),
    bar_action(
        "Copy as Markdown Link",
        "copy markdown link url",
        "link",
        Some("copyMarkdown"),
        Shortcut::CopyLink(true),
    ),
    bar_action(
        "Pin or Unpin Tab",
        "pin unpin tab",
        "stack",
        Some("pinTab"),
        Shortcut::Pin,
    ),
    bar_action(
        "Duplicate Tab",
        "duplicate copy tab",
        "tab",
        Some("duplicateTab"),
        Shortcut::Duplicate,
    ),
    bar_action(
        "Mute or Unmute Tab",
        "mute unmute sound audio silence tab",
        "muted",
        Some("muteTab"),
        Shortcut::Mute,
    ),
    bar_action(
        "Reload",
        "reload refresh page",
        "reload",
        Some("reload"),
        Shortcut::Browser(Navigation::Reload),
    ),
    bar_action(
        "Developer Tools",
        "developer tools devtools inspect console",
        "terminal",
        Some("devTools"),
        Shortcut::DevTools,
    ),
    bar_action(
        "View Source",
        "view source html code page",
        "code",
        Some("viewSource"),
        Shortcut::ViewSource,
    ),
    bar_action(
        "Split View with New Tab",
        "split view new tab side",
        "columns",
        Some("splitNew"),
        Shortcut::Command(Command::Split),
    ),
    bar_action(
        "Exit Split View",
        "exit close split view",
        "columns",
        Some("exitSplit"),
        Shortcut::ExitSplit,
    ),
    bar_action(
        "Peek at an Address",
        "peek preview open address",
        "external",
        None,
        Shortcut::Command(Command::Peek),
    ),
    bar_action(
        "Next Space",
        "next space switch",
        "arrowRight",
        Some("nextSpace"),
        Shortcut::StepSpace(false),
    ),
    bar_action(
        "Previous Space",
        "previous space switch",
        "arrowLeft",
        Some("previousSpace"),
        Shortcut::StepSpace(true),
    ),
];

/// `EMPTY_ACTIONS` (chromium/shell/lib/commandBar.ts:14): the actions an empty bar shows.
const EMPTY_ACTIONS: usize = 2;
/// `actionItems(q).slice(0, 4)` (chromium/shell/ui/app.ts:1305): the actions among a query's
/// results.
const QUERY_ACTIONS: usize = 4;

fn action_for(run: Shortcut) -> Option<&'static BarAction> {
    ACTIONS.iter().find(|action| action.run == run)
}

/// The icon of a row that isn't a page.
fn target_icon(target: &Target) -> &'static str {
    match target {
        Target::Copy(_) => "copy",
        Target::Action(run) => action_for(*run).map_or("zap", |action| action.icon),
        _ => "search",
    }
}

/// `matchingActions` and the `action` scores (chromium/shell/ui/app.ts:1212-1220,
/// lib/commandBar.ts:36-46): every typed word starts a word of the label or keywords, and a
/// label that starts with the query ranks first. Fewer than `min_length` UTF-16 units match
/// nothing.
fn action_score(action: &BarAction, query: &str, min_length: usize) -> Option<u16> {
    let query = trim_input(query).to_lowercase();
    let label = action.label.to_lowercase();
    let keywords = action.keywords.to_lowercase();
    let words = || label.split_whitespace().chain(keywords.split_whitespace());
    (!query.is_empty()
        && query.encode_utf16().count() >= min_length
        && query
            .split_whitespace()
            .all(|prefix| words().any(|word| word.starts_with(prefix))))
    .then_some(if label.starts_with(&query) {
        1400
    } else {
        1250
    })
}

/// The actions `window` can run now, matched against `text`: every one in an empty Actions
/// bar, the two `empty_bar` ones in an empty bar, and the best four for a query.
fn action_rows(
    state: &Controller,
    window: &WindowId,
    text: &str,
    actions: bool,
) -> Vec<Suggestion> {
    let Some(shown) = ShownWindow::read(state, window) else {
        return Vec::new();
    };
    // The backup holds regular state, so only a regular window offers it back.
    let offered = ACTIONS.iter().filter(|action| {
        shown.can_run(action.run)
            && (action.run != Shortcut::RestoreBeforeImport || state.store.has_before_import())
    });
    let row = |action: &BarAction, score| Suggestion {
        target: Target::Action(action.run),
        label: action.label.into(),
        detail: String::new(),
        kind: "Action",
        score,
        recency: 0,
    };
    let empty = trim_input(text).is_empty();
    if empty {
        return offered
            .filter(|action| actions || action.empty_bar)
            .take(if actions { usize::MAX } else { EMPTY_ACTIONS })
            .map(|action| row(action, 0))
            .collect();
    }
    let mut rows: Vec<_> = offered
        .filter_map(|action| {
            Some(row(
                action,
                action_score(action, text, if actions { 1 } else { 2 })?,
            ))
        })
        .collect();
    rows.sort_by_key(|row| Reverse(row.score));
    if !actions {
        rows.truncate(QUERY_ACTIONS);
    }
    rows
}

impl CommandBar {
    fn refresh_matches(&mut self, state: &Controller, window: &WindowId, may_complete: bool) {
        let Some(sources) = state.command_sources(window) else {
            return;
        };
        let previous = std::mem::replace(
            &mut self.matches,
            CommandMatches::for_window(
                state,
                sources,
                window,
                &self.typed,
                may_complete,
                self.mode,
                self.actions,
            ),
        );
        self.matches.keep_remote(&previous);
        self.show_selected_completion();
        self.scroll.scroll_to_item(0);
    }

    fn show_selected_completion(&mut self) {
        if self
            .matches
            .rows
            .get(self.matches.selected)
            .is_some_and(|row| row.kind == "Completion")
            && let Some(completion) = &self.matches.completion
        {
            self.input = TextField::new(self.typed.clone(), false);
            self.input
                .append_selected(&completion.text[self.typed.to_lowercase().len()..]);
        } else if self.input.text() != self.typed {
            self.input = TextField::new(self.typed.clone(), false);
        }
    }
}

impl Shell {
    pub(crate) fn open_command(
        &mut self,
        mode: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if mode == Command::OpenSettings {
            if matches!(self.overlay, Overlay::Settings(_)) {
                self.dismiss_overlay(window, cx);
                return true;
            }
            return self.toggle_settings(SettingsTab::General, window, cx);
        }
        let next = {
            let state = self.state.borrow();
            // `incognito::refusal` turns the Flechinha away, so its bar does not open.
            if mode == Command::Flechinha && state.model.is_incognito(&self.id) {
                return false;
            }
            let Some(sources) = state.command_sources(&self.id) else {
                return false;
            };
            let Some(tab) = location(sources.tabs, &self.id, mode) else {
                return false;
            };
            let input = TextField::new(
                tab.map_or_else(String::new, |tab| tab.url.clone()),
                tab.is_some(),
            );
            let matches = CommandMatches::for_window(
                &state,
                sources,
                &self.id,
                input.text(),
                false,
                mode,
                false,
            );
            CommandBar {
                mode,
                actions: false,
                typed: input.text().into(),
                input,
                matches,
                request_task: None,
                scroll: ScrollHandle::new(),
            }
        };
        self.show_overlay(Overlay::Command(next), window, cx);
        self.start_suggestions(cx);
        true
    }

    fn start_suggestions(&mut self, cx: &mut Context<Self>) {
        let Overlay::Command(bar) = &mut self.overlay else {
            return;
        };
        bar.request_task = None;
        let Some(request) = bar.matches.request.clone() else {
            return;
        };
        let next_send = self.last_suggestion_sent.map(|last| {
            last + Duration::from_millis(suggestions::SUGGEST_INTERVAL_MS.unsigned_abs())
        });
        bar.request_task = Some(cx.spawn(async move |shell, cx| {
            if let Some(due) = next_send.filter(|due| *due > Instant::now()) {
                Timer::at(due).await;
            }
            let work = shell
                .update(cx, |this, cx| {
                    if !this.current_suggestion_request(&request) {
                        return None;
                    }
                    this.last_suggestion_sent = Some(Instant::now());
                    let client = this.suggestion_client.clone();
                    let request = request.clone();
                    Some(cx.background_spawn(async move { client.load(&request).await }))
                })
                .ok()
                .flatten();
            let Some(work) = work else { return };
            let terms = work.await.unwrap_or_default();
            let _ = shell.update(cx, |this, cx| {
                if this.current_suggestion_request(&request)
                    && let Overlay::Command(bar) = &mut this.overlay
                    && bar.matches.complete(&request, terms)
                {
                    bar.scroll.scroll_to_item(bar.matches.selected);
                    cx.notify();
                }
            });
        }));
    }

    fn current_suggestion_request(&self, request: &Arc<SuggestionRequest>) -> bool {
        matches!(&self.overlay, Overlay::Command(bar) if bar.matches.expects(request))
            && request.matches_settings(&self.state.borrow().model.core.settings)
    }

    pub(crate) fn sync_command_preferences(&mut self, cx: &mut Context<Self>) {
        let Overlay::Command(bar) = &mut self.overlay else {
            return;
        };
        let state = self.state.borrow();
        let settings = &state.model.core.settings;
        if bar.matches.engine == settings.search_engine
            && bar.matches.suggestions_enabled == settings.search_suggestions_enabled()
        {
            return;
        }
        bar.refresh_matches(&state, &self.id, bar.matches.completion.is_some());
        drop(state);
        self.start_suggestions(cx);
    }

    pub(crate) fn command_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Overlay::Command(bar) = &mut self.overlay else {
            return;
        };
        match command_key(stroke) {
            key @ (Key::Up | Key::Down) => {
                bar.matches.select(key);
                bar.show_selected_completion();
                if !bar.matches.rows.is_empty() {
                    bar.scroll.scroll_to_item(bar.matches.selected);
                }
            }
            Key::Submit => {
                if let Some(row) = bar.matches.rows.get(bar.matches.selected) {
                    let target = row.target.clone();
                    self.choose_suggestion(&target, window, cx);
                }
                return;
            }
            Key::Dismiss => {
                self.dismiss_overlay(window, cx);
                return;
            }
            // Tab keeps focus in the bar; in an empty bar it searches the actions.
            Key::Tab => {
                if bar.typed.is_empty() && !bar.actions && bar.mode.offers_actions() {
                    bar.actions = true;
                    bar.refresh_matches(&self.state.borrow(), &self.id, false);
                }
            }
            Key::Edit
                if bar.actions && stroke.key == "backspace" && bar.input.text().is_empty() =>
            {
                bar.actions = false;
                bar.refresh_matches(&self.state.borrow(), &self.id, false);
                self.start_suggestions(cx);
            }
            Key::Edit => {
                let previous = bar.input.text().to_owned();
                bar.input.key(stroke, cx);
                if bar.input.text() != previous {
                    let may_complete = bar.input.at_end()
                        && !matches!(stroke.key.as_str(), "backspace" | "delete")
                        && (stroke.key_char.is_some() || stroke.key == "v");
                    bar.typed = bar.input.text().into();
                    bar.refresh_matches(&self.state.borrow(), &self.id, may_complete);
                    self.start_suggestions(cx);
                }
            }
        }
        cx.notify();
    }

    fn choose_suggestion(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Command(bar) = &self.overlay else {
            return;
        };
        if !bar.matches.rows.iter().any(|row| &row.target == target) {
            return;
        }
        let mode = bar.mode;
        let url = match target {
            Target::Action(shortcut) => {
                let shortcut = *shortcut;
                self.dismiss_overlay(window, cx);
                self.invoke_shortcut(shortcut, window, cx);
                return;
            }
            Target::Archived(url) => {
                let url = url.clone();
                // `reopen` opens a new page where the bar's own mode would replace the current one.
                let action = match mode {
                    Command::Split => Action::SplitNew(url),
                    Command::Peek => Action::Peek(url),
                    Command::Flechinha => Action::OpenFlechinha(url),
                    Command::NewTab | Command::EditLocation | Command::OpenSettings => {
                        Action::Open(url)
                    }
                };
                self.navigate_from_overlay(action, window, cx);
                return;
            }
            Target::Copy(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.dismiss_overlay(window, cx);
                return;
            }
            Target::Tab(tab, _) if matches!(bar.mode, Command::NewTab | Command::EditLocation) => {
                let action = Action::Select(tab.clone());
                self.navigate_from_overlay(action, window, cx);
                return;
            }
            Target::Tab(_, url) | Target::Address(url) | Target::Search(url) => url.clone(),
        };
        let action = match mode {
            Command::NewTab => Action::Open(url),
            Command::EditLocation => Action::Navigate(url),
            Command::Split => Action::SplitNew(url),
            Command::Peek => Action::Peek(url),
            Command::Flechinha => Action::OpenFlechinha(url),
            Command::OpenSettings => unreachable!("Settings has its own overlay"),
        };
        self.navigate_from_overlay(action, window, cx);
    }

    /// A tab result's own icon, or for an address or archived tab the icon of a tab on its
    /// site in the partition of the window's shown tab.
    fn target_favicon(&self, target: &Target) -> Option<Arc<gpui::RenderImage>> {
        let state = self.state.borrow();
        match target {
            Target::Tab(id, _) => state.tab_favicon(id),
            Target::Address(url) | Target::Archived(url) => {
                let partition = state.model.partition(state.model.active(&self.id)?)?;
                state.site_favicon(&partition, url)
            }
            _ => None,
        }
    }

    pub(crate) fn command_panel(&self, bar: &CommandBar, cx: &Context<Self>) -> Div {
        let keybindings = self.state.borrow().model.core.settings.keybindings.clone();
        // An action shows its shortcut where a page shows its kind (`actionItem`'s `hint`).
        let hint = |target: &Target| {
            let Target::Action(run) = target else {
                return None;
            };
            let keys = shortcuts::display_keys(
                shortcuts::keys_for(&keybindings, action_for(*run)?.shortcut?, platform()),
                platform(),
            );
            (!keys.is_empty()).then_some(keys)
        };
        // #palette-results .item (ui/styles.css:1315-1397).
        let rows = bar.matches.rows.iter().enumerate().map(|(index, row)| {
            let target = row.target.clone();
            let selected = index == bar.matches.selected;
            let icon_color = if selected {
                palette().selection_text
            } else {
                palette().text_2
            };
            let chip = match &row.target {
                Target::Tab(_, url) | Target::Address(url) | Target::Archived(url) => self
                    .site_icon(
                        self.target_favicon(&row.target),
                        url,
                        (
                            tokens::FAVICON,
                            tokens::FAVICON_RADIUS,
                            tokens::FAVICON_LETTER,
                        ),
                    ),
                target => icon(target_icon(target), tokens::ICON, icon_color).into_any_element(),
            };
            let hint = hint(&row.target);
            let go = match (&row.target, bar.mode) {
                (Target::Tab(..), Command::Split) => Some("Split with Tab"),
                (Target::Tab(..), Command::Flechinha) => None,
                (Target::Tab(..), _) => Some("Switch to Tab"),
                _ => None,
            };
            div()
                .id(("suggestion", index))
                .flex()
                .flex_none()
                .gap(px(tokens::SPACE_3))
                .items_center()
                .min_w_0()
                .h(px(tokens::PALETTE_ITEM))
                .pl(px(tokens::PALETTE_ITEM_PAD_LEFT))
                .pr(px(tokens::SPACE_3))
                .rounded(px(tokens::RADIUS_MD))
                .cursor_pointer()
                .when(selected, |row| {
                    row.bg(palette().selection)
                        .text_color(palette().selection_text)
                })
                .when(!selected, |row| {
                    row.hover(|style| style.bg(palette().surface_2))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.choose_suggestion(&target, window, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(tokens::PALETTE_ITEM_ICON))
                        .rounded(px(tokens::RADIUS_SM))
                        .bg(if selected {
                            palette().selected_chip
                        } else {
                            palette().surface_2
                        })
                        .child(chip),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(row.label.clone())
                        .when(!row.detail.is_empty(), |label| {
                            label.child(
                                div()
                                    .flex_none()
                                    .text_color(if selected {
                                        palette().selected_sub
                                    } else {
                                        palette().text_3
                                    })
                                    .child(format!(" — {}", row.detail)),
                            )
                        })
                        .flex()
                        .overflow_hidden(),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(tokens::SPACE_2))
                        .text_size(px(tokens::TEXT_SM))
                        .text_color(if selected {
                            palette().selected_sub
                        } else {
                            palette().text_3
                        })
                        .map(|right| match hint {
                            Some(keys) => right.child(kbd(keys)),
                            None => right.child(go.unwrap_or(row.kind)),
                        })
                        .when(go.is_some(), |go| {
                            go.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(tokens::PALETTE_GO_ARROW))
                                    .rounded(px(tokens::RADIUS_SM))
                                    .bg(if selected {
                                        palette().selected_chip
                                    } else {
                                        palette().surface_2
                                    })
                                    .child(icon("arrowRight", tokens::PALETTE_GO_ICON, icon_color)),
                            )
                        }),
                )
        });
        // The field shows the selected row's icon (`palette-icon`).
        let field_icon = match bar
            .matches
            .rows
            .get(bar.matches.selected)
            .map(|row| &row.target)
        {
            Some(target @ (Target::Tab(_, url) | Target::Address(url) | Target::Archived(url))) => {
                self.site_icon(
                    self.target_favicon(target),
                    url,
                    (
                        tokens::PALETTE_ICON,
                        tokens::FAVICON_RADIUS,
                        tokens::FAVICON_LETTER,
                    ),
                )
            }
            Some(target) => {
                icon(target_icon(target), tokens::PALETTE_ICON, palette().text_3).into_any_element()
            }
            None => icon(
                if bar.actions { "zap" } else { "search" },
                tokens::PALETTE_ICON,
                palette().text_3,
            )
            .into_any_element(),
        };
        let (mode, placeholder) = if bar.actions {
            ("Actions", "Search actions…")
        } else {
            (bar.mode.label(), "Search or Enter URL…")
        };
        overlay_panel()
            .p_0()
            .gap_0()
            .overflow_hidden()
            .rounded(px(tokens::RADIUS_XL))
            .child(
                // .palette-field (ui/styles.css:1251-1301).
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(tokens::SPACE_3))
                    .h(px(tokens::PALETTE_FIELD))
                    .pl(px(tokens::PALETTE_FIELD_PAD_LEFT))
                    .pr(px(tokens::SPACE_4))
                    .border_b(px(0.5))
                    .border_color(palette().surface_line)
                    .text_color(palette().text_3)
                    .child(field_icon)
                    .child(
                        bar.input
                            .render(placeholder)
                            .flex_1()
                            .h_auto()
                            .px_0()
                            .bg(gpui::transparent_black())
                            .shadow(Vec::new())
                            .text_color(palette().text)
                            .text_size(px(tokens::TEXT_XL))
                            .font_weight(FontWeight::NORMAL),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(tokens::PALETTE_MODE_PAD_X))
                            .py(px(tokens::PALETTE_MODE_PAD_Y))
                            .rounded(px(tokens::RADIUS_SM))
                            .bg(palette().surface_2)
                            .text_size(px(tokens::TEXT_XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette().text_2)
                            .child(mode),
                    )
                    .child(
                        div()
                            .id("close-overlay")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.dismiss_overlay(window, cx);
                            }))
                            .child(kbd("Esc")),
                    ),
            )
            .child(
                div()
                    .id("suggestions")
                    .flex()
                    .flex_col()
                    .h(px(
                        5. * tokens::PALETTE_ITEM + 2. * tokens::PALETTE_RESULTS_PAD
                    ))
                    .p(px(tokens::PALETTE_RESULTS_PAD))
                    .overflow_y_scroll()
                    .track_scroll(&bar.scroll)
                    .children(rows),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flecha_core::{Placement, SpaceId};
    use gpui::Keystroke;

    fn bar_rows(
        state: &Controller,
        window: &WindowId,
        query: &str,
        mode: Command,
        actions: bool,
    ) -> Vec<Suggestion> {
        let sources = state.command_sources(window).unwrap();
        CommandMatches::for_window(state, sources, window, query, false, mode, actions).rows
    }

    fn action_labels(rows: &[Suggestion]) -> Vec<&str> {
        rows.iter()
            .filter(|row| matches!(row.target, Target::Action(_)))
            .map(|row| row.label.as_str())
            .collect()
    }

    #[test]
    fn restore_action_is_offered_for_a_matching_query_only_when_a_backup_exists()
    -> anyhow::Result<()> {
        let profile = tempfile::tempdir()?;
        let state = Controller::open_test_profile(profile.path())?;
        let command = |query: &str| {
            let window = &state.model.core.focused_window().id;
            bar_rows(&state, window, query, Command::NewTab, false)
                .into_iter()
                .find(|row| row.target == Target::Action(Shortcut::RestoreBeforeImport))
        };
        assert!(command("restore spaces").is_none());
        state
            .store
            .write(&flecha_core::storage::set_aside_before_import(
                &state.model.core,
            )?)?;
        assert!(command("s").is_none());
        assert!(command("restore tabs").is_none());
        assert_eq!(
            command("restore spaces").unwrap().label,
            RESTORE_BEFORE_IMPORT
        );
        assert_eq!(
            command("revert import").unwrap().label,
            RESTORE_BEFORE_IMPORT
        );
        Ok(())
    }

    #[test]
    fn actions_fill_an_empty_bar_rank_among_pages_and_list_alone_after_tab() {
        let profile = tempfile::tempdir().unwrap();
        let mut state = Controller::open_test_profile(profile.path()).unwrap();
        let window = state.model.core.focused_window().id.clone();
        state
            .act(&window, Action::Open("https://pin.example/".into()))
            .unwrap();

        let empty = bar_rows(&state, &window, "", Command::NewTab, false);
        assert_eq!(action_labels(&empty), ["New Space", "New Flechinha Window"]);
        assert!(matches!(
            empty.last().unwrap().target,
            Target::Action(Shortcut::Command(Command::Flechinha))
        ));
        // A Peek or Flechinha bar picks a page, so it offers no actions.
        assert!(action_labels(&bar_rows(&state, &window, "", Command::Peek, false)).is_empty());

        // "pin" starts the label of Pin or Unpin Tab, which ranks above the typed search.
        let pin = bar_rows(&state, &window, "pin", Command::NewTab, false);
        let action = pin
            .iter()
            .position(|row| row.target == Target::Action(Shortcut::Pin))
            .unwrap();
        let search = pin.iter().position(|row| row.kind == "Search").unwrap();
        assert!(action < search);
        // Among pages one letter matches no action; on their own it does.
        assert!(action_labels(&bar_rows(&state, &window, "s", Command::NewTab, false)).is_empty());
        let alone = bar_rows(&state, &window, "s", Command::NewTab, true);
        assert!(!alone.is_empty());
        assert!(
            alone
                .iter()
                .all(|row| matches!(row.target, Target::Action(_)))
        );
        assert_eq!(alone[0].score, 1400);

        let all = bar_rows(&state, &window, "", Command::NewTab, true);
        assert!(
            all.iter()
                .all(|row| matches!(row.target, Target::Action(_)))
        );
        let labels = action_labels(&all);
        for offered in ["New Window", "Copy Link", "Developer Tools", "View Source"] {
            assert!(labels.contains(&offered), "{offered}");
        }
        // Nothing is split and no backup exists.
        for absent in ["Exit Split View", RESTORE_BEFORE_IMPORT] {
            assert!(!labels.contains(&absent), "{absent}");
        }

        let private = state.model.open_incognito_window();
        state
            .model
            .open(&private, "https://private.example/".into(), true)
            .unwrap();
        let labels = bar_rows(&state, &private, "", Command::NewTab, true);
        let labels = action_labels(&labels);
        assert!(labels.contains(&"Copy Link"));
        for absent in ["New Space", "Edit Space", "Next Space", "Show Archive"] {
            assert!(!labels.contains(&absent), "{absent}");
        }
    }

    #[test]
    fn archived_tabs_match_and_reopen_as_pages_but_never_in_incognito() {
        let profile = tempfile::tempdir().unwrap();
        let mut state = Controller::open_test_profile(profile.path()).unwrap();
        let window = state.model.core.focused_window().id.clone();
        let closed = TabId::new("closed");
        let mut tab = Tab::new("https://archived.example/notes", 0);
        tab.title = "Quarterly notes".into();
        let space = state.model.core.focused_window().active_space_id.clone();
        assert!(state.model.core.create_tab(
            closed.clone(),
            tab,
            Placement::Today {
                window: &window,
                space: &space,
            },
            None,
        ));
        let _ = state.model.core.close_tab(&closed, 1);
        assert_eq!(state.model.core.archive().len(), 1);

        let rows = bar_rows(&state, &window, "quarterly", Command::NewTab, false);
        let row = rows.iter().find(|row| row.kind == "Archived").unwrap();
        assert_eq!(
            row.target,
            Target::Archived("https://archived.example/notes".into())
        );
        assert_eq!(row.score, 1100);
        assert_eq!(row.detail, "archived.example/notes");
        assert!(
            bar_rows(&state, &window, "", Command::NewTab, false)
                .iter()
                .all(|row| row.kind != "Archived")
        );

        let private = state.model.open_incognito_window();
        assert!(
            bar_rows(&state, &private, "quarterly", Command::NewTab, false)
                .iter()
                .all(|row| row.kind != "Archived")
        );
    }

    #[test]
    fn every_action_has_a_shipping_icon_and_a_known_shortcut() {
        for action in ACTIONS {
            assert!(
                crate::icons::ICONS
                    .iter()
                    .any(|(name, _)| *name == action.icon),
                "{}",
                action.label
            );
            if let Some(id) = action.shortcut {
                assert_eq!(
                    crate::shortcuts::shortcut_for_id(id),
                    Some(action.run),
                    "{id}"
                );
            }
        }
    }

    #[test]
    fn candidates_rank_local_sources_and_deduplicate_history() {
        let window = WindowId::new("main");
        let start = SpaceId::new("start");
        let other = SpaceId::new("other");
        let mut core = BrowserState::new(start.clone(), window.clone());
        core.add_space(other.clone());
        let favorite = TabId::new("favorite");
        let pinned = TabId::new("pinned");
        let today = TabId::new("today");
        for (id, url, title, placement) in [
            (
                &favorite,
                "https://favorite.example/",
                "Project favorite",
                Placement::Favorite,
            ),
            (
                &pinned,
                "https://pinned.example/",
                "Project pinned",
                Placement::Pinned {
                    space: &other,
                    folder: None,
                },
            ),
            (
                &today,
                "https://today.example/",
                "Project today",
                Placement::Today {
                    window: &window,
                    space: &start,
                },
            ),
        ] {
            let mut tab = Tab::new(url, 0);
            tab.title = title.into();
            assert!(core.create_tab(id.clone(), tab, placement, None));
        }
        let mut history = HistoryStore::in_memory().unwrap();
        for (url, title, at) in [
            ("https://today.example/", "Project today", 3),
            ("https://old.example/", "Project history", 1),
            ("https://old.example/", "Project history", 2),
            ("https://recent.example/", "Project recent", 4),
        ] {
            history.record_visit(url, title, at).unwrap();
        }
        let (rows, _) = suggestions_for(
            Sources::regular(&core, Some(&history)),
            None,
            "project",
            false,
        );
        assert_eq!(
            rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
            [
                "Favorite", "Pinned", "Open tab", "Search", "History", "History"
            ]
        );
        assert_eq!(rows[4].label, "Project recent");
        assert_eq!(
            suggestions_for(
                Sources::regular(&core, Some(&history)),
                None,
                "example.com",
                false
            )
            .0[0]
                .target,
            Target::Address("https://example.com".into())
        );
    }

    #[test]
    fn padded_credential_addresses_never_offer_a_search() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        let history = HistoryStore::in_memory().unwrap();
        for userinfo in ["user", "user:pass"] {
            for separator in [
                "@\u{FEFF}",
                "@\u{FEFF}\t",
                "@\t\u{FEFF}",
                "@\u{FEFF}\u{200A}",
                "@\u{FEFF}\u{00A0}",
                "@\u{FEFF}\u{3000}",
                "@\u{200A}",
                "@\u{2009}",
                "@\u{00A0}",
                "@\u{3000}",
                "@\u{202F}",
                "@\u{2028}",
                "@\t",
                "@ ",
                "@\u{85}\u{200B}",
                "\u{FEFF}\u{2009}@\u{2060}\u{202F}",
            ] {
                for host in ["host.example", "localhost", "127.0.0.1", "[::1]"] {
                    for suffix in ["", ":8080", "/path", ":8080/path"] {
                        let input = format!("{userinfo}{separator}{host}{suffix}");
                        let (rows, _) = suggestions_for(
                            Sources::regular(&core, Some(&history)),
                            None,
                            &input,
                            false,
                        );
                        assert!(
                            rows.iter().all(|row| matches!(row.target, Target::Copy(_))),
                            "{input:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ordinary_at_sign_phrases_offer_a_search() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        for query in [
            "rust @ 2026",
            "dinner @ 7pm",
            "lunch @ 12",
            "price @ 5%",
            "tweet @ someone",
            "meet @ noon tomorrow",
            "contact@ example",
            "weather @ stockholm",
            "lunch @ noon?",
            "anyone free @ noon?",
            "dinner @ 7pm?",
            "is it raining @ stockholm?",
            "meet @ 7/8",
            "apples @ $2/lb",
            "rate @ 5%/yr",
            "who is free @ 3pm/4pm",
        ] {
            let (rows, _) = suggestions_for(Sources::regular(&core, None), None, query, false);
            assert_eq!(rows.len(), 1, "{query:?}");
            assert_eq!(
                rows[0].target,
                Target::Search(core.settings.search_engine.search_url(query))
            );
        }
    }

    #[test]
    fn developer_aliases_with_credentials_offer_no_external_targets() {
        let core = BrowserState::new(SpaceId::new("space"), WindowId::new("main"));
        for query in [
            "mdn user:pass@\u{feff}host.example",
            "mdn user:pass@host.example",
            "mdn user@ \u{feff}localhost",
        ] {
            let (rows, _) = suggestions_for(Sources::regular(&core, None), None, query, false);
            assert!(
                rows.iter().all(|row| matches!(row.target, Target::Copy(_))),
                "{query:?}"
            );
        }
    }

    #[test]
    fn command_overlay_selects_and_wraps_and_rejects_non_main_windows() {
        let main = WindowId::new("main");
        let mut core = BrowserState::new(SpaceId::new("space"), main.clone());
        core.open_today(
            TabId::new("tab"),
            Tab::new("https://example.com", 0),
            &main,
            None,
            true,
        );
        assert!(location(&core, &WindowId::new("popup"), Command::NewTab).is_none());
        assert!(location(&core, &WindowId::new("popup"), Command::EditLocation).is_none());
        assert!(matches!(
            location(&core, &main, Command::NewTab),
            Some(None)
        ));
        assert_eq!(
            location(&core, &main, Command::EditLocation)
                .flatten()
                .unwrap()
                .url,
            "https://example.com"
        );
        let up = || command_key(&Keystroke::parse("up").unwrap());
        let down = || command_key(&Keystroke::parse("down").unwrap());
        assert_eq!(selection_after(0, 3, up()), 2);
        assert_eq!(selection_after(2, 3, down()), 0);
        assert_eq!(selection_after(0, 3, down()), 1);
        assert_eq!(selection_after(0, 0, up()), 0);
        assert_eq!(
            command_key(&Keystroke::parse("enter").unwrap()),
            Key::Submit
        );
        assert_eq!(
            command_key(&Keystroke::parse("escape").unwrap()),
            Key::Dismiss
        );
    }
}
