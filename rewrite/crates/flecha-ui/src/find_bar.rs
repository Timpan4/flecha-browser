use flecha_cef::{BrowserKey, FindEvent, FindResult};
use gpui::{
    ClickEvent, Context, Div, InteractiveElement as _, Keystroke, MouseButton, MouseDownEvent,
    MouseUpEvent, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
    px,
};

use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Shell, icon_button, turned_icon_button};

pub(crate) struct FindSession {
    pub browser: BrowserKey,
    bar: FindBar,
}

#[derive(Default)]
struct FindBar {
    input: TextField,
    active: i32,
    total: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindAction {
    Search { forward: bool, next: bool },
    Clear,
    Close,
}

impl FindBar {
    fn key(&mut self, stroke: &Keystroke, edit: impl FnOnce(&mut TextField)) -> Option<FindAction> {
        match stroke.key.as_str() {
            "escape" => Some(FindAction::Close),
            "enter" => self.step(!stroke.modifiers.shift),
            _ => {
                let previous = self.input.text().to_owned();
                edit(&mut self.input);
                if self.input.text() == previous {
                    return None;
                }
                self.active = 0;
                self.total = 0;
                Some(if self.input.text().is_empty() {
                    FindAction::Clear
                } else {
                    FindAction::Search {
                        forward: true,
                        next: false,
                    }
                })
            }
        }
    }

    fn step(&self, forward: bool) -> Option<FindAction> {
        (!self.input.text().is_empty()).then_some(FindAction::Search {
            forward,
            next: true,
        })
    }

    fn update(&mut self, result: FindResult) {
        if result.count >= 0 {
            self.total = result.count;
        }
        if result.active_match_ordinal >= 0 {
            self.active = result.active_match_ordinal;
        }
        if self.total == 0 && result.final_update {
            self.active = 0;
        }
    }

    fn count(&self) -> String {
        format!("{}/{}", self.active, self.total)
    }
}

impl Shell {
    pub(crate) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(browser) = self.active_browser() else {
            return false;
        };
        let find = self.find.get_or_insert_with(|| FindSession {
            browser,
            bar: FindBar::default(),
        });
        find.bar.input = TextField::new(find.bar.input.text().to_owned(), true);
        self.focus.focus(window, cx);
        self.sync_page_focus(window, cx);
        cx.notify();
        true
    }

    pub(crate) fn stop_find(&mut self) {
        if let Some(find) = self.find.take() {
            let state = self.state.borrow();
            if state.engine.key(&find.browser.tab).as_ref() == Some(&find.browser) {
                state.engine.stop_finding(&find.browser.tab, true);
            }
        }
    }

    pub(crate) fn receive_find(&mut self, event: FindEvent, cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find
            && find.browser == event.key
            && !find.bar.input.text().is_empty()
        {
            find.bar.update(event.result);
            cx.notify();
        }
    }

    pub(crate) fn find_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(find) = &mut self.find else {
            return false;
        };
        if self.overlay.is_open() || (!self.focus.is_focused(window) && stroke.key != "escape") {
            return false;
        }
        if let Some(action) = find.bar.key(stroke, |input| input.key(stroke, cx)) {
            self.apply_find(action, window, cx);
        } else {
            cx.notify();
        }
        true
    }

    fn apply_find(&mut self, action: FindAction, window: &mut Window, cx: &mut Context<Self>) {
        if action == FindAction::Close {
            self.stop_find();
            self.page_focus.focus(window, cx);
            self.sync_page_focus(window, cx);
        } else if let Some(find) = &self.find {
            let state = self.state.borrow();
            match action {
                FindAction::Search { forward, next } => {
                    if !next {
                        state.engine.stop_finding(&find.browser.tab, true);
                    }
                    // CEF counts without selecting when findNext is false.
                    state.engine.find(
                        &find.browser.tab,
                        find.bar.input.text(),
                        forward,
                        false,
                        true,
                    );
                }
                FindAction::Clear => state.engine.stop_finding(&find.browser.tab, true),
                FindAction::Close => unreachable!(),
            }
        }
        cx.notify();
    }

    pub(crate) fn find_panel(&self, find: &FindSession, cx: &Context<Self>) -> Div {
        let step = |id, label, forward| {
            // The chevron points right; a quarter turn either way points it up or down.
            let turn = if forward { 0.25 } else { -0.25 };
            turned_icon_button(id, "chevron", label, turn).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if let Some(action) = this.find.as_ref().and_then(|find| find.bar.step(forward)) {
                    this.focus.focus(window, cx);
                    this.sync_page_focus(window, cx);
                    this.apply_find(action, window, cx);
                }
            }))
        };
        // The shipping find bar is Chromium's own, so this one is drawn as a `.popover`
        // (ui/styles.css:1010-1022) with `.icon-button`s.
        div()
            .absolute()
            .top(px(tokens::SPACE_2))
            .right(px(tokens::SPACE_2))
            .max_w_full()
            .occlude()
            .flex()
            .items_center()
            .gap(px(tokens::SPACE_1))
            .p(px(tokens::SPACE_1))
            .rounded(px(tokens::RADIUS_LG))
            .bg(palette().surface)
            .text_color(palette().text)
            .shadow(palette().shadow_pop())
            .on_any_mouse_down(|_: &MouseDownEvent, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation()
            })
            .on_mouse_up(MouseButton::Middle, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation()
            })
            .on_mouse_up(MouseButton::Right, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation()
            })
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .child(
                find.bar
                    .input
                    .render("Find in page")
                    .w(px(170.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.sync_page_focus(window, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .min_w(px(36.))
                    .text_size(px(tokens::TEXT_XS))
                    .font_family(tokens::MONO)
                    .text_color(palette().text_2)
                    .child(find.bar.count()),
            )
            .child(step("find-previous", "Previous match", false))
            .child(step("find-next", "Next match", true))
            .child(icon_button("find-close", "close", "Close find bar").on_click(cx.listener(
                |this, _: &ClickEvent, window, cx| {
                    this.apply_find(FindAction::Close, window, cx);
                },
            )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_bar_starts_searches_steps_merges_updates_and_requests_stop() {
        let mut bar = FindBar::default();
        let key = |key| Keystroke::parse(key).unwrap();
        assert_eq!(bar.count(), "0/0");
        assert_eq!(bar.key(&key("enter"), |_| panic!()), None);
        assert_eq!(
            bar.key(&key("a"), |field| *field =
                TextField::new("a".into(), false)),
            Some(FindAction::Search {
                forward: true,
                next: false
            })
        );
        bar.update(FindResult {
            count: 3,
            active_match_ordinal: 1,
            final_update: true,
        });
        assert_eq!(bar.count(), "1/3");
        assert_eq!(
            bar.key(&key("enter"), |_| panic!()),
            Some(FindAction::Search {
                forward: true,
                next: true
            })
        );
        bar.update(FindResult {
            count: -1,
            active_match_ordinal: 2,
            final_update: false,
        });
        assert_eq!(bar.count(), "2/3");
        assert_eq!(
            bar.key(&key("shift-enter"), |_| panic!()),
            Some(FindAction::Search {
                forward: false,
                next: true
            })
        );
        assert_eq!(
            bar.key(&key("backspace"), |field| *field = TextField::default()),
            Some(FindAction::Clear)
        );
        assert_eq!(bar.count(), "0/0");
        assert_eq!(
            bar.key(&key("escape"), |_| panic!()),
            Some(FindAction::Close)
        );
    }
}
