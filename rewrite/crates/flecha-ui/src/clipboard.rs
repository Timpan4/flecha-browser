//! Copy, Cut and Paste in web pages through GPUI's clipboard.
//!
//! On Wayland, Chromium's own clipboard cannot reach the desktop while GPUI owns the focused
//! window, so the shell reads and writes the clipboard for the page. See `flecha_cef`'s
//! clipboard module for the page side. Elsewhere Chromium's clipboard works and keeps rich
//! content and the page's clipboard events, so the shell leaves page edits to Chromium. On X11
//! it has to: once a page script makes Chromium the clipboard owner, a GPUI read waits on
//! Chromium on the thread they share, and a shell paste would stall.
//!
//! A copy or cut reads the page asynchronously, and a busy page answers late. Keys and
//! clipboard edits that follow it for the same browser wait in a [`Hold`] until the read's
//! text is on the clipboard, then reach the page in their original order, as they would in
//! Chromium. Typing after Ctrl+X therefore cannot replace the selection before the cut reads
//! it, and Ctrl+V after Ctrl+X pastes the cut text.

use std::collections::VecDeque;

use cef::BrowserHost;
use flecha_cef::{BrowserKey, ClipboardEdit, MenuFrame};
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

/// Page input that waits behind a pending selection read.
pub(crate) enum Held {
    Key {
        stroke: Keystroke,
        up: bool,
        repeat: bool,
        altgr: bool,
    },
    /// A paste from the keyboard, with its key.
    Paste(Keystroke),
    /// A paste from the page menu, delivered only if its frame is still focused.
    MenuPaste(MenuFrame),
    /// A copy or cut from the keyboard, with its key and repeat flag.
    Read(ClipboardEdit, Keystroke, bool),
    /// A cut from the page menu, delivered only if its frame is still focused.
    MenuCut(MenuFrame),
    /// A page menu Copy of the selection the menu opened on.
    Write(String),
}

/// A browser's pending selection read and the input that arrived after it. It exists while
/// either does.
#[derive(Default)]
pub(crate) struct Hold {
    reading: bool,
    queue: VecDeque<Held>,
}

/// Gives the page a whole key the shell took. The shell swallows the real release.
fn forward(host: &BrowserHost, stroke: &Keystroke, repeat: bool) {
    input::key(host, stroke, false, repeat, false);
    input::key(host, stroke, true, false, false);
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
    /// the page gets the whole keystroke after all, with its repeat flag. Paste without
    /// clipboard text, and a copy or cut the engine could not start, leave the key to the page.
    /// Behind a pending read, the shortcut waits its turn.
    pub(crate) fn page_clipboard_key(
        &self,
        focused: Option<&BrowserKey>,
        stroke: &Keystroke,
        repeat: bool,
        window: &Window,
        cx: &App,
    ) -> bool {
        let (Some(key), Some(edit)) = (focused, shortcut(stroke)) else {
            return false;
        };
        if !self.shell_clipboard(window) {
            return false;
        }
        let read = match edit {
            PageClipboard::Paste => None,
            PageClipboard::Copy => Some(ClipboardEdit::Copy),
            PageClipboard::Cut => Some(ClipboardEdit::Cut),
        };
        let held = match read {
            None => Held::Paste(stroke.clone()),
            Some(edit) => Held::Read(edit, stroke.clone(), repeat),
        };
        if self.state.borrow_mut().hold(key, held) {
            return true;
        }
        let Some(edit) = read else {
            // The platform read can block, so it runs before the controller is borrowed.
            let Some(text) = text(cx) else {
                return false;
            };
            return self.state.borrow().engine.paste_text(&key.tab, &text);
        };
        self.state
            .borrow_mut()
            .read_selection(key, edit, Some((stroke.clone(), repeat)))
    }
}

impl Controller {
    /// Queues `input` for `key`'s browser behind its pending read or earlier held input.
    /// Returns `false`, keeping nothing, when the browser holds nothing.
    pub(crate) fn hold(&mut self, key: &BrowserKey, input: Held) -> bool {
        let Some(hold) = self.held.get_mut(key) else {
            return false;
        };
        hold.queue.push_back(input);
        true
    }

    /// Whether input for `key`'s browser has to wait.
    pub(crate) fn holding(&self, key: &BrowserKey) -> bool {
        self.held.contains_key(key)
    }

    /// Starts a selection read for `key`'s browser, whose later input then waits for it. A
    /// keyboard read carries its key, which the page gets when nothing may be copied. Returns
    /// whether the read was sent.
    pub(crate) fn read_selection(
        &mut self,
        key: &BrowserKey,
        edit: ClipboardEdit,
        stroke: Option<(Keystroke, bool)>,
    ) -> bool {
        let sent = self.engine.copy_selection(&key.tab, edit, move |host| {
            if let Some((stroke, repeat)) = &stroke {
                forward(host, stroke, *repeat);
            }
        });
        if sent {
            self.held.entry(key.clone()).or_default().reading = true;
        }
        sent
    }

    /// Puts each selection a page copied on GPUI's clipboard, then gives each browser no
    /// longer reading the input it held.
    pub(crate) fn poll_clipboard(&mut self, cx: &mut App) {
        while let Some(copied) = self.engine.next_copied_selection() {
            if self.verify {
                crate::verify::page_copy(&copied.tab, copied.text.is_some());
            }
            if let Some(text) = copied.text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            if let Some(key) = self.engine.key(&copied.tab)
                && let Some(hold) = self.held.get_mut(&key)
            {
                hold.reading = false;
            }
        }
        self.release_held(cx);
    }

    fn release_held(&mut self, cx: &mut App) {
        // A closed or replaced browser answers no read, and its held input has no page to go
        // to, so it is dropped.
        let engine = &self.engine;
        self.held
            .retain(|key, _| engine.key(&key.tab).as_ref() == Some(key));
        let released: Vec<_> = self
            .held
            .iter()
            .filter(|(_, hold)| !hold.reading)
            .map(|(key, _)| key.clone())
            .collect();
        for key in released {
            while let Some(input) = self
                .held
                .get_mut(&key)
                .and_then(|hold| hold.queue.pop_front())
            {
                if self.deliver(&key, input, cx) {
                    break;
                }
            }
            if self
                .held
                .get(&key)
                .is_some_and(|hold| !hold.reading && hold.queue.is_empty())
            {
                self.held.remove(&key);
            }
        }
    }

    /// Gives held `input` to `key`'s browser. Returns `true` when it started another read, which
    /// the rest of the input waits for.
    fn deliver(&mut self, key: &BrowserKey, input: Held, cx: &mut App) -> bool {
        let Some(host) = self.engine.host(&key.tab) else {
            return false;
        };
        match input {
            Held::Key {
                stroke,
                up,
                repeat,
                altgr,
            } => input::key(&host, &stroke, up, repeat, altgr),
            Held::Paste(stroke) => match text(cx) {
                Some(text) => {
                    self.engine.paste_text(&key.tab, &text);
                }
                None => forward(&host, &stroke, false),
            },
            Held::MenuPaste(frame) => {
                if frame.is_focused()
                    && let Some(text) = text(cx)
                {
                    self.engine.paste_text(&key.tab, &text);
                }
            }
            Held::Read(edit, stroke, repeat) => {
                if self.read_selection(key, edit, Some((stroke.clone(), repeat))) {
                    return true;
                }
                forward(&host, &stroke, repeat);
            }
            Held::MenuCut(frame) => {
                if frame.is_focused() && self.read_selection(key, ClipboardEdit::Cut, None) {
                    return true;
                }
            }
            Held::Write(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
        }
        false
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
