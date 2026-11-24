//! HTML page fullscreen (the Fullscreen API: video players, slides, games).
//!
//! Alloy-style CEF leaves the transition to the client (`cef_display_handler_capi.h`): while
//! a window's keyboard pane is in fullscreen, the window goes native fullscreen and shows that
//! page alone, beside the sidebar only while "Sidebar in page fullscreen" is on. Esc and
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

/// A window's native fullscreen on behalf of its page. The window manager applies a request
/// later, so the page may leave fullscreen before the window has entered it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NativeFullscreen {
    /// Flecha holds no native fullscreen. Native fullscreen the user chose stays theirs.
    #[default]
    Idle,
    /// Flecha asked for native fullscreen and the window has not entered it yet.
    Requested,
    /// The window is in native fullscreen because Flecha asked.
    Held,
}

impl NativeFullscreen {
    /// The next state, from whether the page wants fullscreen and whether the window is in it,
    /// and whether to toggle the window's native fullscreen now.
    pub(crate) fn next(self, wanted: bool, window_fullscreen: bool) -> (Self, bool) {
        match (self, wanted, window_fullscreen) {
            (Self::Idle, true, false) => (Self::Requested, true),
            (Self::Idle, _, _) => (Self::Idle, false),
            (Self::Requested, true, true) => (Self::Held, false),
            // The page left before the window entered: undo the entry once it lands.
            (Self::Requested, false, true) | (Self::Held, false, true) => (Self::Idle, true),
            (Self::Requested, _, false) => (Self::Requested, false),
            // The user already took the window out of native fullscreen.
            (Self::Held, false, false) => (Self::Idle, false),
            (Self::Held, true, _) => (Self::Held, false),
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

    /// Runs renders of (page wants fullscreen, window is fullscreen) and returns the window's
    /// toggles and the final state.
    fn native(renders: &[(bool, bool)]) -> (Vec<bool>, NativeFullscreen) {
        let mut state = NativeFullscreen::Idle;
        let toggles = renders
            .iter()
            .map(|&(wanted, window)| {
                let (next, toggle) = state.next(wanted, window);
                state = next;
                toggle
            })
            .collect();
        (toggles, state)
    }

    #[test]
    fn a_late_native_entry_after_the_page_left_is_undone() {
        // Enter, Esc before the window manager answers, then the entry lands.
        let (toggles, state) = native(&[(true, false), (false, false), (false, true)]);
        assert_eq!(toggles, [true, false, true]);
        assert_eq!(state, NativeFullscreen::Idle);
        // Nothing more once the exit lands.
        assert_eq!(state.next(false, false), (NativeFullscreen::Idle, false));
    }

    #[test]
    fn native_fullscreen_follows_the_page_without_repeating_requests() {
        let (toggles, state) = native(&[(true, false), (true, false), (true, true), (false, true)]);
        assert_eq!(toggles, [true, false, false, true]);
        assert_eq!(state, NativeFullscreen::Idle);
        // The exit has not landed yet: no second toggle.
        assert_eq!(state.next(false, true), (NativeFullscreen::Idle, false));
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
