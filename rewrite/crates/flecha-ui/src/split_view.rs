//! The split view's divider (chrome/browser/ui/views/frame/multi_contents_resize_area.cc, with
//! patch 0019 making the gap between the panes the handle): dragging it shares the page area
//! between the panes as the pointer moves, and the share is saved when the button comes up.
//! A double click shares the area evenly again.

use std::cell::Cell;
use std::rc::Rc;

use flecha_core::{SpaceId, TabId, clamp_split_ratio};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Bounds, Context, DispatchPhase, Div, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Styled as _, canvas,
    div, px,
};

use crate::theme::palette;
use crate::tokens;
use crate::view::Shell;
use crate::{Action, dispatch};

/// Chromium's resize handle: `kHandleResizeAxisSize` by `kHandleOffAxisSize`, rounded by
/// half its width (multi_contents_resize_area.cc).
const HANDLE_WIDTH: f32 = 4.;
const HANDLE_HEIGHT: f32 = 24.;

/// A divider drag under way.
pub(crate) struct SplitDrag {
    /// The split it shares out: its Space and pair of tabs.
    split: Option<ShownSplit>,
    start: f64,
    pub(crate) ratio: f64,
}

type ShownSplit = (SpaceId, [TabId; 2]);

/// The first pane's share when the pointer is at `x` in a split area spanning `area`: the
/// divider's middle follows the pointer.
fn ratio_at(area: Bounds<Pixels>, x: Pixels) -> Option<f64> {
    let room = f64::from(area.size.width) - f64::from(tokens::SPLIT_GAP);
    (room > 0.).then(|| {
        let left = f64::from(x - area.origin.x) - f64::from(tokens::SPLIT_GAP) / 2.;
        clamp_split_ratio(left / room)
    })
}

impl Shell {
    /// Records where the split area is, and follows the pointer while the divider is dragged.
    pub(crate) fn split_input(&self, cx: &Context<Self>) -> impl IntoElement {
        let shell = cx.entity();
        let area = self.split_area.clone();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                area.set(bounds);
                let view = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        let Some(drag) = &this.split_drag else {
                            return;
                        };
                        // Another split came to the front, by a key or another window: the
                        // drag was for the one that left.
                        if drag.split != this.shown_split() {
                            this.split_drag = None;
                            cx.notify();
                            return;
                        }
                        if event.pressed_button == Some(MouseButton::Left) {
                            if let Some(drag) = &mut this.split_drag
                                && let Some(ratio) =
                                    ratio_at(this.split_area.get(), event.position.x)
                            {
                                drag.ratio = ratio;
                            }
                            cx.notify();
                        } else {
                            this.finish_split_drag(cx);
                        }
                        cx.stop_propagation();
                    });
                });
                let view = shell.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        view.update(cx, |this, cx| {
                            if this.split_drag.is_some() {
                                this.finish_split_drag(cx);
                                cx.stop_propagation();
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The split the window shows, which `SetSplitRatio` changes.
    fn shown_split(&self) -> Option<ShownSplit> {
        let state = self.state.borrow();
        let core = state.model.state(&self.id).unwrap_or(&state.model.core);
        let window = core.window(&self.id)?;
        let pair = window.spaces.get(&window.active_space_id)?.split.clone()?;
        Some((window.active_space_id.clone(), pair))
    }

    /// Cancels an active divider drag as soon as another split or Space comes to the front.
    pub(crate) fn prune_split_drag(&mut self) {
        if self
            .split_drag
            .as_ref()
            .is_some_and(|drag| drag.split != self.shown_split())
        {
            self.split_drag = None;
        }
    }

    /// The share a drag of the shown split has reached, before it is saved.
    pub(crate) fn dragged_split_ratio(&self) -> Option<f64> {
        self.split_drag
            .as_ref()
            .filter(|drag| drag.split == self.shown_split())
            .map(|drag| drag.ratio)
    }

    /// Saves a drag's share, if it moved and its split is still the one shown, and ends the
    /// drag.
    pub(crate) fn finish_split_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.split_drag.take() {
            if drag.ratio != drag.start && drag.split == self.shown_split() {
                dispatch(&self.state, &self.id, Action::SetSplitRatio(drag.ratio), cx);
            }
            cx.notify();
        }
    }

    /// The divider over the gap right of the first pane, whose share is `ratio`.
    pub(crate) fn split_divider(&self, ratio: f64, cx: &Context<Self>) -> impl IntoElement {
        let dragging = self.split_drag.is_some();
        div()
            .id("split-divider")
            .group("split-divider")
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(-tokens::SPLIT_GAP))
            .w(px(tokens::SPLIT_GAP))
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .cursor_col_resize()
            .child(
                div()
                    .w(px(HANDLE_WIDTH))
                    .h(px(HANDLE_HEIGHT))
                    .rounded(px(HANDLE_WIDTH / 2.))
                    .when(dragging, |handle| handle.bg(palette().text_3))
                    .group_hover("split-divider", |handle| handle.bg(palette().text_3)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count == 2 {
                        this.split_drag = None;
                        dispatch(&this.state, &this.id, Action::SetSplitRatio(0.5), cx);
                        return;
                    }
                    this.split_drag = Some(SplitDrag {
                        split: this.shown_split(),
                        start: ratio,
                        ratio,
                    });
                    cx.notify();
                }),
            )
    }

    /// Keeps the pointer a resize cursor, and the pages from taking it, while the divider moves.
    pub(crate) fn split_drag_layer(&self) -> Option<Div> {
        self.split_drag
            .as_ref()
            .map(|_| div().absolute().inset_0().occlude().cursor_col_resize())
    }
}

/// Where the split area was last drawn, for a drag to measure the pointer against.
pub(crate) type SplitArea = Rc<Cell<Bounds<Pixels>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn the_divider_follows_the_pointer_within_the_shell_range() {
        let area = Bounds::new(point(px(100.), px(0.)), size(px(808.), px(600.)));
        // 800 to share after the gap: the divider's middle at 400 in is an even split.
        assert_eq!(ratio_at(area, px(100. + 404.)), Some(0.5));
        assert_eq!(ratio_at(area, px(100. + 4. + 200.)), Some(0.25));
        assert_eq!(ratio_at(area, px(100.)), Some(0.2));
        assert_eq!(ratio_at(area, px(2000.)), Some(0.8));
        let narrow = Bounds::new(point(px(0.), px(0.)), size(px(8.), px(600.)));
        assert_eq!(ratio_at(narrow, px(4.)), None);
    }
}
