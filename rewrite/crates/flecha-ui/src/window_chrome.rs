//! A Flecha window's own title bar. On Linux the shipping frame has none of the platform's: its
//! sidebar and Flechinha toolbars hold the close, minimize and maximize circles
//! (ui/windowControls.ts) and move the window (`-webkit-app-region: drag`, ui/styles.css:51-57).
//! The rewrite asks GPUI for client-side decorations and does the same where it gets them.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Decorations, Div, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    ResizeEdge, StatefulInteractiveElement as _, Styled as _, Tiling, TitlebarOptions, Window,
    WindowDecorations, WindowOptions, div, prelude::FluentBuilder as _, px,
};

use crate::icons::icon;
use crate::tokens;
use crate::view::Shell;
use crate::{Action, dispatch};

/// What draws a window's title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TitleBar {
    /// The platform's (server-side decorations, an X11 desktop without a compositor, a
    /// popup), or none in full screen: Flecha draws no controls.
    Platform,
    /// macOS, whose traffic lights GPUI keeps over the toolbar's start: the toolbar leaves their
    /// room (`.traffic-lights`, ui/styles.css:75-77).
    Room,
    /// Flecha: it draws the controls, moves the window from its toolbars and resizes it from
    /// the edges that are not tiled.
    Drawn(Tiling),
}

impl TitleBar {
    /// Read on every frame: the compositor decides, and may change its mind (tiling, a
    /// compositor that starts or stops).
    pub(crate) fn of(window: &Window) -> Self {
        if window.is_fullscreen() {
            return Self::Platform;
        }
        if cfg!(target_os = "macos") {
            return Self::Room;
        }
        match window.window_decorations() {
            Decorations::Client { tiling } => Self::Drawn(tiling),
            Decorations::Server => Self::Platform,
        }
    }
}

/// A main, incognito or Flechinha window: no title bar of the platform's where the platform
/// lets Flecha draw its own. GPUI falls back to the platform's on X11 without a compositor.
pub(crate) fn client_decorated(options: WindowOptions) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..TitlebarOptions::default()
        }),
        window_decorations: Some(WindowDecorations::Client),
        ..options
    }
}

#[derive(Clone, Copy)]
enum Control {
    Close,
    Minimize,
    Maximize,
}

impl Shell {
    /// `.window-controls` (ui/windowControls.css), `width` wide with `pad` before the circles,
    /// or the room macOS draws its own in.
    pub(crate) fn window_controls(
        &self,
        title_bar: TitleBar,
        (width, pad): (f32, f32),
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let room = div().flex().flex_none().items_center().w(px(width));
        match title_bar {
            TitleBar::Platform => None,
            TitleBar::Room => Some(room.into_any_element()),
            TitleBar::Drawn(_) => {
                let group = gpui::SharedString::new_static("window-controls");
                let idle = !window.is_window_active();
                let maximized = window.is_maximized();
                Some(
                    room.id("window-controls")
                        .group(group.clone())
                        .occlude()
                        .pl(px(pad))
                        .child(window_control(Control::Close, idle, maximized).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                dispatch(&this.state, &this.id, Action::CloseWindow, cx);
                            }),
                        ))
                        .child(
                            window_control(Control::Minimize, idle, maximized)
                                .on_click(|_: &ClickEvent, window, _| window.minimize_window()),
                        )
                        .child(
                            window_control(Control::Maximize, idle, maximized)
                                .on_click(|_: &ClickEvent, window, _| window.zoom_window()),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    /// Lets the pointer move the window from `bar`'s empty room, where nothing in it occludes
    /// the bar, and a double click there maximize or restore it.
    pub(crate) fn window_drag(&self, bar: Div, title_bar: TitleBar, cx: &Context<Self>) -> Div {
        if !matches!(title_bar, TitleBar::Drawn(_)) {
            return bar;
        }
        bar.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, _| {
                this.moving = event.click_count == 1;
                if event.click_count == 2 {
                    window.zoom_window();
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, _| this.moving = false),
        )
        // The move starts with the drag, as Zed's title bar does, so a double click still
        // reaches the bar on Wayland.
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, _| {
            if event.pressed_button == Some(MouseButton::Left) && std::mem::take(&mut this.moving) {
                window.start_window_move();
            }
        }))
    }
}

/// The window's edges that resize it: the margin around the page (`kFlechaGap`), which takes no
/// page input and sits inside the sidebar's padding, on each side that is not tiled.
pub(crate) fn resize_edges(title_bar: TitleBar) -> Vec<Div> {
    let TitleBar::Drawn(tiling) = title_bar else {
        return Vec::new();
    };
    let band = px(tokens::PAGE_GAP);
    let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
    [
        (top && left, ResizeEdge::TopLeft),
        (top && right, ResizeEdge::TopRight),
        (bottom && left, ResizeEdge::BottomLeft),
        (bottom && right, ResizeEdge::BottomRight),
        (top, ResizeEdge::Top),
        (bottom, ResizeEdge::Bottom),
        (left, ResizeEdge::Left),
        (right, ResizeEdge::Right),
    ]
    .into_iter()
    .filter(|(resizable, _)| *resizable)
    .map(|(_, edge)| {
        let handle = div().absolute().occlude();
        let handle = match edge {
            ResizeEdge::Top | ResizeEdge::Bottom => handle
                .left(band)
                .right(band)
                .h(band)
                .cursor(CursorStyle::ResizeUpDown),
            ResizeEdge::Left | ResizeEdge::Right => handle
                .top(band)
                .bottom(band)
                .w(band)
                .cursor(CursorStyle::ResizeLeftRight),
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                handle.size(band).cursor(CursorStyle::ResizeUpLeftDownRight)
            }
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                handle.size(band).cursor(CursorStyle::ResizeUpRightDownLeft)
            }
        };
        let handle = match edge {
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight => handle.top_0(),
            _ => handle.bottom_0(),
        };
        let handle = match edge {
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft => handle.left_0(),
            ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight => handle.right_0(),
            ResizeEdge::Top | ResizeEdge::Bottom => handle,
        };
        handle.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.start_window_resize(edge);
            cx.stop_propagation();
        })
    })
    .collect()
}

/// The close circle alone, for a Flechinha under the platform's title bar.
pub(crate) fn close_control() -> gpui::Stateful<Div> {
    window_control(Control::Close, false, false)
}

/// `.window-control` (ui/windowControls.css:4-74): a 12px circle in a 20px target, its glyph
/// shown while the pointer is over the controls, grey in a window that is not focused.
fn window_control(control: Control, idle: bool, maximized: bool) -> gpui::Stateful<Div> {
    let (id, label, color, glyph) = match control {
        Control::Close => (
            "window-close",
            "Close window",
            ".window-control.close",
            "window-close",
        ),
        Control::Minimize => (
            "window-minimize",
            "Minimize",
            ".window-control.minimize",
            "window-minimize",
        ),
        Control::Maximize if maximized => (
            "window-maximize",
            "Restore",
            ".window-control.maximize",
            "window-restore",
        ),
        Control::Maximize => (
            "window-maximize",
            "Maximize",
            ".window-control.maximize",
            "window-maximize",
        ),
    };
    let group = gpui::SharedString::new_static("window-controls");
    let inset = tokens::WINDOW_CONTROL_INSET;
    let color = tokens::color(color, false);
    let idle_color = tokens::color(".window-control idle", false);
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(tokens::WINDOW_CONTROL))
        .cursor_pointer()
        .aria_label(label)
        .child(
            div()
                .absolute()
                .top(px(inset))
                .left(px(inset))
                .size(px(tokens::WINDOW_CONTROL - 2. * inset))
                .rounded_full()
                .bg(if idle { idle_color } else { color })
                .when(idle, |circle| {
                    circle.group_hover(group.clone(), move |circle| circle.bg(color))
                })
                .shadow(tokens::shadow_window_control()),
        )
        .child(
            icon(
                glyph,
                tokens::WINDOW_CONTROL_GLYPH,
                tokens::color(".window-control svg stroke", false),
            )
            .opacity(0.)
            .group_hover(group, |glyph| glyph.opacity(1.)),
        )
}
