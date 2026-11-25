//! HTML page fullscreen (the Fullscreen API: video players, slides, games).
//!
//! Alloy-style CEF leaves the transition to the client (`cef_display_handler_capi.h`): while
//! a window's keyboard pane is in fullscreen, the window shows that page alone. With "Sidebar
//! in page fullscreen" on and the sidebar docked, the page stays in the docked page area and
//! the window keeps its size, leaving the desktop's panels visible (patch 0075). Otherwise the
//! window goes native fullscreen. Hiding the sidebar moves the page to the screen, and showing
//! it again brings the window back while the page stays in fullscreen (patch 0077). Esc and
//! leaving the page (switching tabs, closing a Peek) end it through
//! [`flecha_cef::Engine::exit_fullscreen`].

use std::collections::BTreeMap;

use flecha_cef::{BrowserKey, Engine, Owner};
use flecha_core::{Settings, TabId, WindowId};

use crate::Controller;

/// The pages in HTML fullscreen, each with the generation of the browser that entered it, so a
/// replaced browser's late event can't end its successor's fullscreen.
#[derive(Default)]
pub(crate) struct PageFullscreen(BTreeMap<TabId, u64>);

impl PageFullscreen {
    fn changed(&mut self, tab: &TabId, generation: u64, fullscreen: bool) {
        if fullscreen {
            self.0.insert(tab.clone(), generation);
        } else if self.0.get(tab) == Some(&generation) {
            self.0.remove(tab);
        }
    }

    pub(crate) fn contains(&self, tab: &TabId) -> bool {
        self.0.contains_key(tab)
    }

    /// Forgets the pages whose browser went or was replaced, and returns, forgotten too, those
    /// still in fullscreen that no window shows as its keyboard pane. They must leave it.
    fn sweep(
        &mut self,
        generation: impl Fn(&TabId) -> Option<u64>,
        shown: impl Fn(&TabId) -> bool,
    ) -> Vec<TabId> {
        let mut hidden = Vec::new();
        self.0.retain(|tab, entered| {
            if generation(tab) != Some(*entered) {
                return false;
            }
            if shown(tab) {
                return true;
            }
            hidden.push(tab.clone());
            false
        });
        hidden
    }
}

/// What a window's page asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageWants {
    /// The page is not in fullscreen.
    Nothing,
    /// The page is in fullscreen beside the kept sidebar, inside the window.
    Window,
    /// The page is in fullscreen alone: the window should be in native fullscreen.
    Screen,
}

impl PageWants {
    pub(crate) fn of(page_fullscreen: bool, keeps_sidebar: bool) -> Self {
        match (page_fullscreen, keeps_sidebar) {
            (false, _) => Self::Nothing,
            (true, true) => Self::Window,
            (true, false) => Self::Screen,
        }
    }
}

/// A window's native fullscreen on behalf of its page.
///
/// The window manager applies a request later or never: X11 sends `_NET_WM_STATE` with
/// `Toggle`, and neither X11 nor Wayland reports a refusal to GPUI. Two toggles in flight cancel
/// out, so as in patch 0077, Flecha sends none while its last one is unanswered and decides
/// again once the window reports the change. The `--verify` harness reads the state by these
/// variant names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) enum NativeFullscreen {
    /// Flecha holds no native fullscreen. Native fullscreen the user chose stays theirs.
    #[default]
    Idle,
    /// Flecha asked for native fullscreen and the window has not entered it yet.
    Requested,
    /// The sidebar came back while the entry was on its way and the page stayed in fullscreen.
    /// Hiding the sidebar again asks for nothing more; if the entry lands with the sidebar
    /// still shown, Flecha undoes it.
    Parked,
    /// The window is in native fullscreen because Flecha asked.
    Held,
    /// The page left fullscreen before the window entered it. If the entry lands, Flecha undoes
    /// it; the page entering again asks afresh.
    Withdrawn,
    /// The user took the window out of native fullscreen while the page stayed in fullscreen.
    /// Flecha asks for nothing more until the page leaves, whatever the sidebar does, and
    /// native fullscreen the user chooses again stays theirs.
    Released,
}

impl NativeFullscreen {
    /// The next state, from what the page wants and whether the window is in native
    /// fullscreen, and whether to toggle the window's native fullscreen now.
    pub(crate) fn next(self, page: PageWants, window_fullscreen: bool) -> (Self, bool) {
        use PageWants::{Nothing, Screen, Window};
        match (self, page, window_fullscreen) {
            (Self::Idle, Screen, false) => (Self::Requested, true),
            (Self::Idle, _, _) => (Self::Idle, false),
            // An entry landed or is held: keep it while the page takes the screen, else leave.
            (Self::Requested | Self::Parked | Self::Withdrawn | Self::Held, Screen, true) => {
                (Self::Held, false)
            }
            (Self::Requested | Self::Parked | Self::Withdrawn | Self::Held, _, true) => {
                (Self::Idle, true)
            }
            // An entry is on its way.
            (Self::Requested | Self::Parked, Screen, false) => (Self::Requested, false),
            (Self::Requested | Self::Parked, Window, false) => (Self::Parked, false),
            (Self::Requested | Self::Parked | Self::Withdrawn, Nothing, false) => {
                (Self::Withdrawn, false)
            }
            (Self::Withdrawn, Screen, false) => (Self::Requested, true),
            (Self::Withdrawn, Window, false) => (Self::Withdrawn, false),
            // The user left native fullscreen.
            (Self::Held | Self::Released, Nothing, false) => (Self::Idle, false),
            (Self::Held | Self::Released, _, false) => (Self::Released, false),
            (Self::Released, Nothing, true) => (Self::Idle, false),
            (Self::Released, _, true) => (Self::Released, false),
        }
    }
}

/// Whether a window whose page is in fullscreen keeps its sidebar. A sidebar the user hid stays
/// hidden.
pub(crate) fn keeps_sidebar(settings: &Settings, sidebar_visible: bool) -> bool {
    sidebar_visible && settings.keeps_sidebar_in_page_fullscreen()
}

impl Controller {
    /// The window's keyboard pane, when its page is in fullscreen.
    pub(crate) fn fullscreen_pane(&self, window: &WindowId) -> Option<(Owner, TabId)> {
        let (owner, tab) = self.model.keyboard_pane(window)?;
        self.fullscreen.contains(tab).then(|| (owner, tab.clone()))
    }

    /// Whether the window's fullscreen page stays beside its sidebar, inside the window. Only
    /// a main or incognito window has a sidebar; a popup or Flechinha page takes the screen.
    pub(crate) fn keeps_sidebar_in_fullscreen(&self, window: &WindowId) -> bool {
        self.model.state(window).is_some_and(|core| {
            core.window(window).is_some()
                && keeps_sidebar(&self.model.core.settings, core.sidebar_visible(window))
        })
    }

    pub(crate) fn page_fullscreen(&mut self, key: &BrowserKey, fullscreen: bool) {
        self.fullscreen
            .changed(&key.tab, key.generation(), fullscreen);
    }

    /// Ends the fullscreen of pages no window shows any more. Returns whether any ended.
    pub(crate) fn sweep_fullscreen(&mut self) -> bool {
        let panes = self
            .windows
            .keys()
            .filter_map(|window| self.model.keyboard_pane(window).map(|(_, tab)| tab.clone()))
            .collect::<Vec<_>>();
        let engine: &Engine = &self.engine;
        let hidden = self.fullscreen.sweep(
            |tab| engine.key(tab).map(|key| key.generation()),
            |tab| panes.contains(tab),
        );
        for tab in &hidden {
            engine.exit_fullscreen(tab);
        }
        !hidden.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fullscreen(entries: &[(&str, u64)]) -> PageFullscreen {
        PageFullscreen(
            entries
                .iter()
                .map(|(tab, generation)| (TabId::new(*tab), *generation))
                .collect(),
        )
    }

    #[test]
    fn the_sidebar_stays_only_while_the_setting_is_on_and_it_was_shown() {
        let mut settings = Settings::default();
        assert!(!keeps_sidebar(&settings, true), "the default hides it");
        settings.sidebar_in_page_fullscreen = Some(true);
        assert!(keeps_sidebar(&settings, true));
        assert!(
            !keeps_sidebar(&settings, false),
            "a hidden sidebar came back"
        );
        settings.sidebar_in_page_fullscreen = Some(false);
        assert!(!keeps_sidebar(&settings, true));
    }

    #[test]
    fn switching_away_ends_fullscreen_and_a_shown_page_keeps_it() {
        let mut pages = fullscreen(&[("video", 1), ("other", 4)]);
        let shown = TabId::new("other");
        let hidden = pages.sweep(
            |tab| Some(if tab.as_str() == "video" { 1 } else { 4 }),
            |tab| tab == &shown,
        );
        assert_eq!(hidden, [TabId::new("video")]);
        assert!(!pages.contains(&TabId::new("video")));
        assert!(pages.contains(&shown));
    }

    #[test]
    fn a_closed_or_replaced_browser_leaves_without_an_exit_request() {
        let mut pages = fullscreen(&[("closed", 1), ("replaced", 2)]);
        let hidden = pages.sweep(|tab| (tab.as_str() == "replaced").then_some(3), |_| false);
        assert!(
            hidden.is_empty(),
            "asked a gone browser to exit: {hidden:?}"
        );
        assert!(pages.0.is_empty());
    }

    use PageWants::{Nothing, Screen, Window};

    /// Runs renders of (what the page wants, window is fullscreen) from `Idle` and returns the
    /// window's toggles and the final state.
    fn run(renders: &[(PageWants, bool)]) -> (Vec<bool>, NativeFullscreen) {
        let mut state = NativeFullscreen::Idle;
        let toggles = renders
            .iter()
            .map(|&(page, window)| {
                let (next, toggle) = state.next(page, window);
                state = next;
                toggle
            })
            .collect();
        (toggles, state)
    }

    /// As [`run`], for a page with no sidebar kept beside it: `true` takes the screen.
    fn native(renders: &[(bool, bool)]) -> (Vec<bool>, NativeFullscreen) {
        let renders = renders
            .iter()
            .map(|&(screen, window)| (if screen { Screen } else { Nothing }, window))
            .collect::<Vec<_>>();
        run(&renders)
    }

    #[test]
    fn hiding_the_sidebar_takes_the_screen_and_showing_it_brings_the_window_back() {
        // tests/native-fullscreen.mjs: fullscreen beside the sidebar, hide it, the entry
        // lands, show it, the exit lands, then the page leaves.
        let (toggles, state) = run(&[
            (Window, false),
            (Screen, false),
            (Screen, true),
            (Window, true),
            (Window, false),
            (Nothing, false),
        ]);
        assert_eq!(toggles, [false, true, false, true, false, false]);
        assert_eq!(state, NativeFullscreen::Idle);
        assert_eq!(
            NativeFullscreen::Held.next(Window, true),
            (NativeFullscreen::Idle, true)
        );
    }

    #[test]
    fn sidebar_toggles_never_stack_native_requests() {
        // Hide, show and hide before the window manager answers: one request, then held.
        let (toggles, state) = run(&[
            (Window, false),
            (Screen, false),
            (Window, false),
            (Screen, false),
            (Screen, true),
        ]);
        assert_eq!(toggles, [false, true, false, false, false]);
        assert_eq!(state, NativeFullscreen::Held);
        // Hide, show, and the entry lands with the sidebar shown: Flecha undoes it.
        let (toggles, state) = run(&[(Screen, false), (Window, false), (Window, true)]);
        assert_eq!(toggles, [true, false, true]);
        assert_eq!(state, NativeFullscreen::Idle);
        // Held, show, hide before the exit lands: nothing until the window reports it left.
        let (toggles, state) = run(&[
            (Screen, false),
            (Screen, true),
            (Window, true),
            (Screen, true),
            (Screen, false),
        ]);
        assert_eq!(toggles, [true, false, true, false, true]);
        assert_eq!(state, NativeFullscreen::Requested);
    }

    #[test]
    fn native_fullscreen_the_user_leaves_stays_theirs_while_the_sidebar_toggles() {
        let (toggles, state) = run(&[
            (Screen, false),
            (Screen, true),
            (Screen, false),
            (Window, false),
            (Screen, false),
        ]);
        assert_eq!(toggles, [true, false, false, false, false]);
        assert_eq!(state, NativeFullscreen::Released);
        assert_eq!(state.next(Nothing, false), (NativeFullscreen::Idle, false));
    }

    #[test]
    fn the_users_own_fullscreen_survives_sidebar_toggles_and_the_page_leaving() {
        // tests/native-fullscreen.mjs: F11, then the page enters, the sidebar hides and shows.
        let (toggles, state) = run(&[
            (Window, true),
            (Screen, true),
            (Window, true),
            (Nothing, true),
        ]);
        assert_eq!(toggles, [false; 4], "toggled the user's fullscreen");
        assert_eq!(state, NativeFullscreen::Idle);
    }

    #[test]
    fn a_late_native_entry_after_the_page_left_is_undone() {
        // Enter, Esc before the window manager answers, then the entry lands.
        let (toggles, state) = native(&[(true, false), (false, false), (false, true)]);
        assert_eq!(toggles, [true, false, true]);
        assert_eq!(state, NativeFullscreen::Idle);
        // Nothing more once the exit lands.
        assert_eq!(state.next(Nothing, false), (NativeFullscreen::Idle, false));
    }

    #[test]
    fn native_fullscreen_follows_the_page_without_repeating_requests() {
        let (toggles, state) = native(&[(true, false), (true, false), (true, true), (false, true)]);
        assert_eq!(toggles, [true, false, false, true]);
        assert_eq!(state, NativeFullscreen::Idle);
        // The exit has not landed yet: no second toggle.
        assert_eq!(state.next(Nothing, true), (NativeFullscreen::Idle, false));
    }

    #[test]
    fn native_fullscreen_the_user_chose_or_left_stays_theirs() {
        let (toggles, _) = native(&[(true, true), (false, true)]);
        assert_eq!(toggles, [false, false], "toggled the user's fullscreen");
        let (toggles, state) =
            native(&[(true, false), (true, true), (true, false), (false, false)]);
        assert_eq!(
            toggles,
            [true, false, false, false],
            "re-entered after the user left"
        );
        assert_eq!(state, NativeFullscreen::Idle);
    }

    #[test]
    fn a_request_the_window_manager_ignored_does_not_block_the_next_entry() {
        // Enter, the window manager ignores it, Esc, then the page enters again.
        let (toggles, state) =
            native(&[(true, false), (false, false), (false, false), (true, false)]);
        assert_eq!(
            toggles,
            [true, false, false, true],
            "the second entry asked for nothing"
        );
        assert_eq!(state, NativeFullscreen::Requested);
        // This time the window enters, and Esc undoes it.
        assert_eq!(state.next(Screen, true), (NativeFullscreen::Held, false));
        assert_eq!(
            NativeFullscreen::Held.next(Nothing, true),
            (NativeFullscreen::Idle, true)
        );
    }

    #[test]
    fn native_fullscreen_the_user_takes_back_during_page_fullscreen_stays_theirs() {
        // Flecha enters, the user leaves native fullscreen and enters it again, then Esc.
        let (toggles, state) = native(&[
            (true, false),
            (true, true),
            (true, false),
            (true, false),
            (true, true),
            (false, true),
        ]);
        assert_eq!(
            toggles,
            [true, false, false, false, false, false],
            "Flecha fought the user's native fullscreen"
        );
        assert_eq!(state, NativeFullscreen::Idle);
        assert_eq!(state.next(Nothing, true), (NativeFullscreen::Idle, false));
    }

    #[test]
    fn only_the_browser_that_entered_fullscreen_can_leave_it() {
        let video = TabId::new("video");
        let mut pages = PageFullscreen::default();
        pages.changed(&video, 2, true);
        assert!(pages.contains(&video));
        pages.changed(&video, 1, false);
        assert!(pages.contains(&video), "an old browser's exit ended it");
        pages.changed(&video, 2, false);
        assert!(!pages.contains(&video));
        assert!(pages.sweep(|_| Some(2), |_| true).is_empty());
    }
}
