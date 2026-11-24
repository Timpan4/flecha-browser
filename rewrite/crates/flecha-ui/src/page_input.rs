use std::ops::Range;

use flecha_cef::{BrowserKey, ImeEdit};
use flecha_core::WindowId;
use gpui::{
    App, Bounds, FocusHandle, InputHandler, Pixels, Point, UTF16Selection, Window, px, size,
};

use crate::clipboard::Held;
use crate::{Controller, Shared};

/// The text input handler installed by a pane's paint can outlive its focus for a frame.
pub(crate) struct PageInput {
    pub state: Shared,
    pub window: WindowId,
    pub browser: BrowserKey,
    pub focus: FocusHandle,
    pub bounds: Bounds<Pixels>,
}

impl PageInput {
    fn is_current(&self, state: &Controller, window: &Window) -> bool {
        window.is_window_active()
            && self.focus.is_focused(window)
            && state
                .model
                .keyboard_pane(&self.window)
                .is_some_and(|(owner, tab)| {
                    tab == &self.browser.tab && state.engine.owner(tab) == Some(&owner)
                })
            && state.engine.ime(&self.browser).is_some()
    }

    fn edit(&self, edit: ImeEdit, window: &Window) {
        let mut state = self.state.borrow_mut();
        if self.is_current(&state, window) {
            state.page_ime(&self.browser, edit);
        }
    }
}

impl InputHandler for PageInput {
    fn commit_preedit_on_pointer_press(&self) -> bool {
        false
    }

    fn accepts_text_input(&mut self, window: &mut Window, _: &mut App) -> bool {
        self.is_current(&self.state.borrow(), window)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        window: &mut Window,
        _: &mut App,
    ) -> Option<UTF16Selection> {
        let state = self.state.borrow();
        if !self.is_current(&state, window) {
            return None;
        }
        state.engine.ime(&self.browser)?.selection()
    }

    fn marked_text_range(&mut self, window: &mut Window, _: &mut App) -> Option<Range<usize>> {
        let state = self.state.borrow();
        if !self.is_current(&state, window) {
            return None;
        }
        state.engine.ime(&self.browser)?.marked_range()
    }

    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        let state = self.state.borrow();
        if !self.is_current(&state, window) {
            return None;
        }
        let text = state
            .engine
            .ime(&self.browser)?
            .marked_text(range.clone())?;
        *adjusted = Some(range);
        Some(text)
    }

    fn replace_text_in_range(
        &mut self,
        replacement: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        _: &mut App,
    ) {
        self.edit(
            ImeEdit::Commit {
                text: text.into(),
                replacement,
            },
            window,
        );
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        replacement: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        window: &mut Window,
        _: &mut App,
    ) {
        self.edit(
            ImeEdit::Mark {
                text: text.into(),
                replacement,
                selection,
            },
            window,
        );
    }

    fn unmark_text(&mut self, window: &mut Window, _: &mut App) {
        self.edit(ImeEdit::Unmark, window);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let state = self.state.borrow();
        if !self.is_current(&state, window) {
            return None;
        }
        let mut bounds = state
            .engine
            .ime(&self.browser)?
            .bounds_for_range(range)
            .unwrap_or_else(|| Bounds::new(Point::default(), size(px(0.), px(0.))));
        bounds.origin += self.bounds.origin;
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        let state = self.state.borrow();
        if !self.is_current(&state, window) {
            return None;
        }
        state
            .engine
            .ime(&self.browser)?
            .character_index(position - self.bounds.origin)
    }
}

impl Controller {
    fn page_ime(&mut self, browser: &BrowserKey, edit: ImeEdit) {
        if let Some(hold) = self.held.get_mut(browser) {
            hold.push(Held::Ime(edit));
        } else {
            self.engine.ime_edit(browser, edit);
        }
    }

    pub(crate) fn blur_page(&mut self, browser: &BrowserKey) {
        if let Some(hold) = self.held.get_mut(browser) {
            hold.discard_ime();
        }
        self.engine.focused(&browser.tab, false);
    }

    pub(crate) fn send_page_key(
        &self,
        browser: &BrowserKey,
        stroke: &gpui::Keystroke,
        up: bool,
        repeat: bool,
        altgr: bool,
    ) {
        let Some(host) = self.engine.host(&browser.tab) else {
            return;
        };
        let composing = self
            .engine
            .ime(browser)
            .is_some_and(|ime| ime.marked_range().is_some());
        if !up && composing {
            if let Some(text) = &stroke.key_char
                && (altgr || stroke.modifiers.is_subset_of(&gpui::Modifiers::shift()))
            {
                // XKB returns completed compose text as a keystroke on both Linux backends.
                // Native IMEs can also deliver an ASCII commit this way on Wayland.
                let mut raw = stroke.clone();
                raw.key_char = None;
                flecha_surface::input::key(&host, &raw, false, repeat, altgr);
                self.engine.ime_edit(
                    browser,
                    ImeEdit::Commit {
                        text: text.clone(),
                        replacement: None,
                    },
                );
                return;
            }
            if stroke.key == "escape" {
                self.engine.ime_edit(browser, ImeEdit::Cancel);
            }
        }
        flecha_surface::input::key(&host, stroke, up, repeat, altgr);
    }
}
