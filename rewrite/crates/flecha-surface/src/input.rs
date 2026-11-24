use cef::{
    BrowserHost, CursorType, ImplBrowserHost as _, KeyEvent, KeyEventType, MouseButtonType,
    MouseEvent, sys::cef_event_flags_t as Flags,
};
use gpui::{
    CursorStyle, Keystroke, Modifiers, MouseButton, MouseMoveEvent, Pixels, Point, ScrollDelta,
    ScrollEvent,
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
    let flags = modifiers(motion.modifiers)
        | motion
            .pressed_button
            .and_then(button)
            .map_or(0, |(_, flag)| flag.0);
    host.send_mouse_move_event(Some(&event(motion.position - origin, flags)), 0);
}

pub fn mouse_leave(host: &BrowserHost, position: Point<Pixels>, state: Modifiers) {
    host.send_mouse_move_event(Some(&event(position, modifiers(state))), 1);
}

pub fn scroll(host: &BrowserHost, scroll: &ScrollEvent, origin: Point<Pixels>) {
    let mut flags = modifiers(scroll.modifiers);
    let (dx, dy) = match scroll.delta {
        ScrollDelta::Pixels(delta) => {
            flags |= Flags::EVENTFLAG_PRECISION_SCROLLING_DELTA.0;
            (f32::from(delta.x), f32::from(delta.y))
        }
        // CEF's legacy wheel API uses Win32 WHEEL_DELTA units for notches.
        ScrollDelta::Lines(delta) => (delta.x * 120.0, delta.y * 120.0),
    };
    host.send_mouse_wheel_event(
        Some(&event(scroll.position - origin, flags)),
        dx.round() as i32,
        dy.round() as i32,
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
