use cef::{
    BrowserHost, CursorType, ImplBrowser as _, ImplBrowserHost as _, KeyEvent, KeyEventType,
    MouseButtonType, MouseEvent, sys::cef_event_flags_t as Flags,
};
use std::cell::Cell;

use gpui::{
    CursorStyle, Keystroke, Modifiers, MouseButton, MouseMoveEvent, Pixels, Point, ScrollDelta,
    ScrollEvent, point,
};

pub fn cursor_style(kind: CursorType) -> CursorStyle {
    match kind {
        CursorType::CROSS | CursorType::CELL => CursorStyle::Crosshair,
        CursorType::HAND => CursorStyle::PointingHand,
        CursorType::IBEAM => CursorStyle::IBeam,
        CursorType::VERTICALTEXT => CursorStyle::IBeamCursorForVerticalLayout,
        CursorType::WAIT => CursorStyle::Wait,
        CursorType::PROGRESS => CursorStyle::Progress,
        CursorType::HELP => CursorStyle::Help,
        CursorType::EASTRESIZE => CursorStyle::ResizeRight,
        CursorType::WESTRESIZE => CursorStyle::ResizeLeft,
        CursorType::NORTHRESIZE => CursorStyle::ResizeUp,
        CursorType::SOUTHRESIZE => CursorStyle::ResizeDown,
        CursorType::EASTWESTRESIZE => CursorStyle::ResizeLeftRight,
        CursorType::NORTHSOUTHRESIZE => CursorStyle::ResizeUpDown,
        CursorType::NORTHWESTRESIZE
        | CursorType::SOUTHEASTRESIZE
        | CursorType::NORTHWESTSOUTHEASTRESIZE => CursorStyle::ResizeUpLeftDownRight,
        CursorType::NORTHEASTRESIZE
        | CursorType::SOUTHWESTRESIZE
        | CursorType::NORTHEASTSOUTHWESTRESIZE => CursorStyle::ResizeUpRightDownLeft,
        CursorType::COLUMNRESIZE => CursorStyle::ResizeColumn,
        CursorType::ROWRESIZE => CursorStyle::ResizeRow,
        CursorType::MOVE | CursorType::DND_MOVE => CursorStyle::Move,
        CursorType::GRAB => CursorStyle::OpenHand,
        CursorType::GRABBING => CursorStyle::ClosedHand,
        CursorType::NOTALLOWED | CursorType::NODROP | CursorType::DND_NONE => {
            CursorStyle::OperationNotAllowed
        }
        CursorType::ALIAS | CursorType::DND_LINK => CursorStyle::DragLink,
        CursorType::COPY | CursorType::DND_COPY => CursorStyle::DragCopy,
        CursorType::CONTEXTMENU => CursorStyle::ContextualMenu,
        CursorType::ZOOMIN => CursorStyle::ZoomIn,
        CursorType::ZOOMOUT => CursorStyle::ZoomOut,
        CursorType::NONE => CursorStyle::Hidden,
        _ => CursorStyle::Arrow,
    }
}

fn modifiers(state: Modifiers) -> u32 {
    let mut flags = Flags::EVENTFLAG_NONE;
    if state.shift {
        flags |= Flags::EVENTFLAG_SHIFT_DOWN;
    }
    if state.control {
        flags |= Flags::EVENTFLAG_CONTROL_DOWN;
    }
    if state.alt {
        flags |= Flags::EVENTFLAG_ALT_DOWN;
    }
    if state.platform {
        flags |= Flags::EVENTFLAG_COMMAND_DOWN;
    }
    flags.0
}

fn button(button: MouseButton) -> Option<(MouseButtonType, Flags)> {
    Some(match button {
        MouseButton::Left => (MouseButtonType::LEFT, Flags::EVENTFLAG_LEFT_MOUSE_BUTTON),
        MouseButton::Middle => (
            MouseButtonType::MIDDLE,
            Flags::EVENTFLAG_MIDDLE_MOUSE_BUTTON,
        ),
        MouseButton::Right => (MouseButtonType::RIGHT, Flags::EVENTFLAG_RIGHT_MOUSE_BUTTON),
        MouseButton::Navigate(_) => return None,
    })
}

fn event(position: Point<Pixels>, modifiers: u32) -> MouseEvent {
    MouseEvent {
        x: f32::from(position.x).round() as i32,
        y: f32::from(position.y).round() as i32,
        modifiers,
    }
}

pub fn click(
    host: &BrowserHost,
    position: Point<Pixels>,
    state: Modifiers,
    mouse_button: MouseButton,
    count: usize,
    up: bool,
) {
    if let (Some((button, flag)), Ok(count)) = (button(mouse_button), i32::try_from(count)) {
        let flags = modifiers(state) | if up { 0 } else { flag.0 };
        host.send_mouse_click_event(Some(&event(position, flags)), button, i32::from(up), count);
    }
}

pub fn mouse_move(host: &BrowserHost, motion: &MouseMoveEvent, origin: Point<Pixels>) {
    let flags = pointer_flags(motion.modifiers, motion.pressed_button);
    host.send_mouse_move_event(Some(&event(motion.position - origin, flags)), 0);
}

pub fn mouse_leave(
    host: &BrowserHost,
    position: Point<Pixels>,
    state: Modifiers,
    pressed_button: Option<MouseButton>,
) {
    let flags = pointer_flags(state, pressed_button);
    host.send_mouse_move_event(Some(&event(position, flags)), 1);
}

fn pointer_flags(state: Modifiers, pressed_button: Option<MouseButton>) -> u32 {
    modifiers(state)
        | pressed_button
            .and_then(button)
            .map_or(0, |(_, flag)| flag.0)
}

/// The pixels Chromium scrolls per wheel line (`kPixelsPerLineStep`, cc/input/scroll_utils.h).
/// GPUI reports 3 lines per wheel notch on Linux and Windows, so a notch sends 120 pixels, as
/// native Chromium does (`ui::MouseWheelEvent::kWheelDelta`).
const PIXELS_PER_LINE: f32 = 40.0;

/// A wheel event for CEF, whose delta is whole pixels.
#[derive(Debug, PartialEq)]
struct Wheel {
    dx: i32,
    dy: i32,
    /// Pixel-exact deltas, as touchpads report. Chromium scrolls them directly; without this
    /// flag it animates the scroll as a wheel notch.
    precise: bool,
}

/// The fraction of a pixel that earlier touchpad scroll events reported but CEF has not received,
/// scoped to the browser that received them.
#[derive(Default)]
pub struct ScrollCarry(Cell<(i32, Point<f32>)>);

impl ScrollCarry {
    fn wheel(&self, target: i32, delta: ScrollDelta) -> Wheel {
        match delta {
            ScrollDelta::Lines(delta) => {
                self.0.set((target, Point::default()));
                let scaled = delta * PIXELS_PER_LINE;
                Wheel {
                    dx: scaled.x.round() as i32,
                    dy: scaled.y.round() as i32,
                    precise: false,
                }
            }
            ScrollDelta::Pixels(delta) => {
                let (prev_target, prev_carry) = self.0.get();
                let carry = if prev_target == target {
                    prev_carry
                } else {
                    Point::default()
                };
                let total = point(f32::from(delta.x), f32::from(delta.y)) + carry;
                // Truncate toward zero so the remainder stays strictly inside (-1, 1) and a
                // subsequent zero-delta event on that axis never oscillates ±1 px.
                let sent = point(total.x.trunc(), total.y.trunc());
                self.0.set((target, total - sent));
                Wheel {
                    dx: sent.x as i32,
                    dy: sent.y as i32,
                    precise: true,
                }
            }
        }
    }
}

pub fn scroll(
    host: &BrowserHost,
    scroll: &ScrollEvent,
    origin: Point<Pixels>,
    carry: &ScrollCarry,
) {
    let target = host.browser().map_or(0, |browser| browser.identifier());
    let wheel = carry.wheel(target, scroll.delta);
    if wheel.dx == 0 && wheel.dy == 0 {
        return;
    }
    let mut flags = modifiers(scroll.modifiers);
    if wheel.precise {
        flags |= Flags::EVENTFLAG_PRECISION_SCROLLING_DELTA.0;
    }
    host.send_mouse_wheel_event(
        Some(&event(scroll.position - origin, flags)),
        wheel.dx,
        wheel.dy,
    );
}

pub fn key(host: &BrowserHost, stroke: &Keystroke, up: bool, repeat: bool, altgr: bool) {
    let mut flags = modifiers(stroke.modifiers);
    if altgr {
        flags &= !(Flags::EVENTFLAG_CONTROL_DOWN.0 | Flags::EVENTFLAG_ALT_DOWN.0);
        flags |= Flags::EVENTFLAG_ALTGR_DOWN.0;
    }
    if repeat {
        flags |= Flags::EVENTFLAG_IS_REPEAT.0;
    }
    let text = stroke.key_char.as_deref().unwrap_or_default();
    let mut key = KeyEvent {
        type_: if up {
            KeyEventType::KEYUP
        } else {
            KeyEventType::RAWKEYDOWN
        },
        modifiers: flags,
        windows_key_code: virtual_key(&stroke.key),
        character: text.encode_utf16().next().unwrap_or_default(),
        unmodified_character: stroke
            .key
            .encode_utf16()
            .next()
            .filter(|_| stroke.key.chars().count() == 1)
            .unwrap_or_default(),
        is_system_key: i32::from(stroke.modifiers.alt && !altgr),
        ..Default::default()
    };
    host.send_key_event(Some(&key));
    if !up && (!stroke.modifiers.control || altgr) && !stroke.modifiers.platform {
        key.type_ = KeyEventType::CHAR;
        for unit in text.encode_utf16() {
            key.character = unit;
            key.windows_key_code = i32::from(unit);
            host.send_key_event(Some(&key));
        }
    }
}

fn virtual_key(key: &str) -> i32 {
    // Win32 virtual-key codes used by CEF on every platform.
    match key {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" => 0x0d,
        "escape" => 0x1b,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2d,
        "delete" => 0x2e,
        ";" => 0xba,
        "=" => 0xbb,
        "," => 0xbc,
        "-" => 0xbd,
        "." => 0xbe,
        "/" => 0xbf,
        "`" => 0xc0,
        "[" => 0xdb,
        "\\" => 0xdc,
        "]" => 0xdd,
        "'" => 0xde,
        _ => match key.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => i32::from(byte.to_ascii_uppercase()),
            _ => 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn a_wheel_notch_scrolls_as_a_native_chromium_notch() {
        // GPUI's Linux backends report a notch as 3 lines (gpui_linux `SCROLL_LINES`).
        let carry = ScrollCarry::default();
        assert_eq!(
            carry.wheel(1, ScrollDelta::Lines(point(0., -3.))),
            Wheel {
                dx: 0,
                dy: -120,
                precise: false
            }
        );
    }

    #[test]
    fn touchpad_pixels_stay_precise_and_keep_their_fractions() {
        let carry = ScrollCarry::default();
        let pixels = |x, y| ScrollDelta::Pixels(point(px(x), px(y)));
        let sent: Vec<_> = (0..4)
            .map(|_| carry.wheel(1, pixels(0., 0.25)).dy)
            .collect();
        assert_eq!(sent, [0, 0, 0, 1]);
        assert!(carry.wheel(1, pixels(0., 0.25)).precise);
        assert_eq!(carry.wheel(1, pixels(0., -1.25)).dy, -1);

        // A 0.5 px movement followed by zero-delta events on that axis must not oscillate.
        assert_eq!(carry.wheel(1, pixels(0.5, 0.)).dx, 0);
        let horizontal: Vec<_> = (0..3).map(|_| carry.wheel(1, pixels(0., -2.)).dx).collect();
        assert_eq!(horizontal, [0, 0, 0]);

        // Touchpad carry must not leak into a wheel notch or across split panes.
        assert_eq!(
            carry.wheel(1, ScrollDelta::Lines(point(0., -3.))),
            Wheel {
                dx: 0,
                dy: -120,
                precise: false
            }
        );
        assert_eq!(carry.wheel(1, pixels(0.75, 0.)).dx, 0);
        assert_eq!(carry.wheel(2, pixels(0.5, 0.)).dx, 0);
    }
}
