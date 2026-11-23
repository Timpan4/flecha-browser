//! winit keys to Windows virtual-key codes, which CEF's `windows_key_code` expects on every
//! platform. Values are the VK_* constants from Microsoft's "Virtual-Key Codes" table, as
//! mirrored in Chromium's ui/events/keycodes/keyboard_codes_posix.h.

use winit::keyboard::{Key, KeyCode, PhysicalKey};

/// Letters take their code from the layout, as Chromium's own X11 and Wayland input does
/// (KeyboardCodeFromXKeyEvent maps the keysym first): on AZERTY the key labelled A is at the
/// physical Q position and must still be VK_A, or Ctrl+A acts as Ctrl+Q. `unmodified` is
/// winit's `key_without_modifiers()`. Every other key uses its physical position.
pub fn windows_key_code(physical: PhysicalKey, unmodified: &Key) -> Option<i32> {
    if let Key::Character(text) = unmodified {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_alphabetic()
        {
            return Some(c.to_ascii_uppercase() as i32);
        }
    }
    physical_key_code(physical)
}

fn physical_key_code(key: PhysicalKey) -> Option<i32> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    use KeyCode::*;
    let vk = match code {
        KeyA => 0x41,
        KeyB => 0x42,
        KeyC => 0x43,
        KeyD => 0x44,
        KeyE => 0x45,
        KeyF => 0x46,
        KeyG => 0x47,
        KeyH => 0x48,
        KeyI => 0x49,
        KeyJ => 0x4A,
        KeyK => 0x4B,
        KeyL => 0x4C,
        KeyM => 0x4D,
        KeyN => 0x4E,
        KeyO => 0x4F,
        KeyP => 0x50,
        KeyQ => 0x51,
        KeyR => 0x52,
        KeyS => 0x53,
        KeyT => 0x54,
        KeyU => 0x55,
        KeyV => 0x56,
        KeyW => 0x57,
        KeyX => 0x58,
        KeyY => 0x59,
        KeyZ => 0x5A,
        Digit0 => 0x30,
        Digit1 => 0x31,
        Digit2 => 0x32,
        Digit3 => 0x33,
        Digit4 => 0x34,
        Digit5 => 0x35,
        Digit6 => 0x36,
        Digit7 => 0x37,
        Digit8 => 0x38,
        Digit9 => 0x39,
        Numpad0 => 0x60,
        Numpad1 => 0x61,
        Numpad2 => 0x62,
        Numpad3 => 0x63,
        Numpad4 => 0x64,
        Numpad5 => 0x65,
        Numpad6 => 0x66,
        Numpad7 => 0x67,
        Numpad8 => 0x68,
        Numpad9 => 0x69,
        NumpadMultiply => 0x6A,
        NumpadAdd => 0x6B,
        NumpadSubtract => 0x6D,
        NumpadDecimal => 0x6E,
        NumpadDivide => 0x6F,
        NumpadEnter => 0x0D,
        F1 => 0x70,
        F2 => 0x71,
        F3 => 0x72,
        F4 => 0x73,
        F5 => 0x74,
        F6 => 0x75,
        F7 => 0x76,
        F8 => 0x77,
        F9 => 0x78,
        F10 => 0x79,
        F11 => 0x7A,
        F12 => 0x7B,
        Backspace => 0x08,
        Tab => 0x09,
        Enter => 0x0D,
        Escape => 0x1B,
        Space => 0x20,
        PageUp => 0x21,
        PageDown => 0x22,
        End => 0x23,
        Home => 0x24,
        ArrowLeft => 0x25,
        ArrowUp => 0x26,
        ArrowRight => 0x27,
        ArrowDown => 0x28,
        PrintScreen => 0x2C,
        Insert => 0x2D,
        Delete => 0x2E,
        ShiftLeft | ShiftRight => 0x10,
        ControlLeft | ControlRight => 0x11,
        AltLeft | AltRight => 0x12,
        Pause => 0x13,
        CapsLock => 0x14,
        SuperLeft => 0x5B,
        SuperRight => 0x5C,
        ContextMenu => 0x5D,
        NumLock => 0x90,
        ScrollLock => 0x91,
        Semicolon => 0xBA,
        Equal => 0xBB,
        Comma => 0xBC,
        Minus => 0xBD,
        Period => 0xBE,
        Slash => 0xBF,
        Backquote => 0xC0,
        BracketLeft => 0xDB,
        Backslash => 0xDC,
        BracketRight => 0xDD,
        Quote => 0xDE,
        IntlBackslash => 0xE2,
        _ => return None,
    };
    Some(vk)
}

pub fn is_keypad(key: PhysicalKey) -> bool {
    use KeyCode::*;
    matches!(
        key,
        PhysicalKey::Code(
            Numpad0
                | Numpad1
                | Numpad2
                | Numpad3
                | Numpad4
                | Numpad5
                | Numpad6
                | Numpad7
                | Numpad8
                | Numpad9
                | NumpadMultiply
                | NumpadAdd
                | NumpadSubtract
                | NumpadDecimal
                | NumpadDivide
                | NumpadEnter
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_follow_the_layout_and_other_keys_their_position() {
        let a = Key::Character("a".into());
        // AZERTY: the A key is at the physical Q position.
        assert_eq!(
            windows_key_code(PhysicalKey::Code(KeyCode::KeyQ), &a),
            Some(0x41)
        );
        // Non-letters keep the physical code (AZERTY's unshifted digit row gives `&`).
        let amp = Key::Character("&".into());
        assert_eq!(
            windows_key_code(PhysicalKey::Code(KeyCode::Digit1), &amp),
            Some(0x31)
        );
        // Swedish ö is not ASCII: physical code (VK_OEM_1 at the Semicolon position).
        let oe = Key::Character("ö".into());
        assert_eq!(
            windows_key_code(PhysicalKey::Code(KeyCode::Semicolon), &oe),
            physical_key_code(PhysicalKey::Code(KeyCode::Semicolon))
        );
    }
}
