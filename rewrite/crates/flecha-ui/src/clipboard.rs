//! Copy, Cut and Paste in web pages through GPUI's clipboard.
//!
//! On Wayland, Chromium's own clipboard cannot reach the desktop while GPUI owns the focused
//! window, so the shell reads and writes the clipboard for the page. See `flecha_cef`'s
//! clipboard module for the page side. Elsewhere Chromium's clipboard works and keeps rich
//! content and the page's clipboard events, so the shell leaves page edits to Chromium. On X11
//! it has to: once a page script makes Chromium the clipboard owner, a GPUI read waits on
//! Chromium on the thread they share, and a shell paste would stall.

use flecha_cef::{BrowserKey, ClipboardEdit};
use flecha_surface::input;
use gpui::{App, ClipboardItem, Keystroke, Window};

use crate::Controller;
use crate::view::Shell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageClipboard {
    Copy,
    Cut,
    Paste,
}

/// The clipboard edit `stroke` asks for: the platform's primary modifier with C, X or V, and
/// with Shift and V for a plain-text paste.
pub(crate) fn shortcut(stroke: &Keystroke) -> Option<PageClipboard> {
    let mods = stroke.modifiers;
    let primary = if cfg!(target_os = "macos") {
        mods.platform && !mods.control
    } else {
        mods.control && !mods.platform
    };
    if !primary || mods.alt || mods.function {
        return None;
    }
    match (stroke.key.as_str(), mods.shift) {
        ("c", false) => Some(PageClipboard::Copy),
        ("x", false) => Some(PageClipboard::Cut),
        ("v", _) => Some(PageClipboard::Paste),
        _ => None,
    }
}

/// GPUI's clipboard text, when there is any to paste.
pub(crate) fn text(cx: &App) -> Option<String> {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .filter(|text| !text.is_empty())
}

impl Shell {
    /// Whether the shell, not Chromium, handles page clipboard edits in `window`.
    pub(crate) fn shell_clipboard(&self, window: &Window) -> bool {
        #[cfg(target_os = "linux")]
        let wayland = flecha_surface::ozone_platform(window) == "wayland";
        #[cfg(not(target_os = "linux"))]
        let wayland = {
            let _ = window;
            false
        };
        wayland || self.state.borrow().shell_clipboard
    }

    /// Runs a clipboard shortcut for the focused page. Returns `true` when the shell took the
    /// key, which then must not reach the page now. Copy and Cut hold it back until the page's
    /// selection is read: with no copyable text, such as nothing selected or a password field,
    /// the page gets the whole keystroke after all. Paste without clipboard text, and a copy or
    /// cut the engine could not start, leave the key to the page.
    pub(crate) fn page_clipboard_key(
        &self,
        focused: Option<&BrowserKey>,
        stroke: &Keystroke,
        window: &Window,
        cx: &App,
    ) -> bool {
        let (Some(key), Some(edit)) = (focused, shortcut(stroke)) else {
            return false;
        };
        if !self.shell_clipboard(window) {
            return false;
        }
        let engine = &self.state.borrow().engine;
        let edit = match edit {
            PageClipboard::Paste => {
                return text(cx).is_some_and(|text| engine.paste_text(&key.tab, &text));
            }
            PageClipboard::Copy => ClipboardEdit::Copy,
            PageClipboard::Cut => ClipboardEdit::Cut,
        };
        let stroke = stroke.clone();
        engine.copy_selection(&key.tab, edit, move |host| {
            for up in [false, true] {
                input::key(host, &stroke, up, false, false);
            }
        })
    }
}

impl Controller {
    /// Puts each selection a page copied on GPUI's clipboard.
    pub(crate) fn poll_clipboard(&mut self, cx: &mut App) {
        while let Some(copied) = self.engine.next_copied_selection() {
            if self.verify {
                crate::verify::page_copy(&copied.tab, copied.text.is_some());
            }
            if let Some(text) = copied.text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_c_x_and_v_are_the_page_clipboard_shortcuts() {
        let primary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let other = if cfg!(target_os = "macos") {
            "ctrl"
        } else {
            "cmd"
        };
        let os = if cfg!(target_os = "macos") {
            gpui::OperatingSystem::Mac
        } else {
            gpui::OperatingSystem::Linux
        };
        let edit = |text: &str| shortcut(&Keystroke::parse_with_os(text, os).unwrap());
        assert_eq!(edit(&format!("{primary}-c")), Some(PageClipboard::Copy));
        assert_eq!(edit(&format!("{primary}-x")), Some(PageClipboard::Cut));
        assert_eq!(edit(&format!("{primary}-v")), Some(PageClipboard::Paste));
        assert_eq!(
            edit(&format!("{primary}-shift-v")),
            Some(PageClipboard::Paste)
        );
        for stroke in [
            "c".to_owned(),
            "v".to_owned(),
            "shift-v".to_owned(),
            format!("{primary}-a"),
            format!("{primary}-shift-c"),
            format!("{primary}-shift-x"),
            // AltGr arrives as Control+Alt on Linux and types characters.
            format!("{primary}-alt-v"),
            format!("{other}-v"),
            format!("{primary}-{other}-v"),
            format!("{primary}-fn-v"),
        ] {
            assert_eq!(edit(&stroke), None, "{stroke}");
        }
    }
}
