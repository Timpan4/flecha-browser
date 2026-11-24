//! Page clipboard edits through the shell's clipboard.
//!
//! Under offscreen rendering Chromium has no window with keyboard focus, and on Wayland the
//! compositor serves clipboard reads and writes only to the focused client. Chromium's own
//! Copy, Cut and Paste therefore never reach the desktop clipboard there. On X11 they do, but a
//! Chromium-owned clipboard stalls the shell's reads, which run on the thread Chromium needs to
//! answer them. The shell owns the focused window, so it reads and writes the clipboard and the
//! engine moves the text in and out of the page: pasted text is committed as typed input, and
//! copied text is read from the page's selection through an internal CDP call.

use cef::*;

use crate::callbacks::Slot;
use crate::{Engine, devtools, services};

/// A page edit that puts the page's selection on the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardEdit {
    Copy,
    Cut,
}

/// The answer to [`Engine::copy_selection`]: the selected text for the shell's clipboard, or
/// `None` when nothing may be copied, such as a password field's selection.
#[derive(Debug, PartialEq, Eq)]
pub struct CopiedSelection {
    pub tab: flecha_core::TabId,
    pub text: Option<String>,
}

/// Reads the selection of the page's focused element. It answers `null`, so nothing reaches
/// the clipboard, for a password field and for a focused frame it cannot read, such as a
/// cross-origin iframe, which could hold one.
const READ_SELECTION: &str = r#"(() => {
  let doc = document;
  let element = doc.activeElement;
  for (;;) {
    while (element?.shadowRoot?.activeElement) element = element.shadowRoot.activeElement;
    if (element?.localName !== 'iframe' && element?.localName !== 'frame') break;
    try {
      doc = element.contentDocument;
    } catch {
      return null;
    }
    if (!doc) return null;
    element = doc.activeElement;
  }
  if (element?.localName === 'input' && element.type === 'password') return null;
  if (element?.localName === 'input' || element?.localName === 'textarea') {
    const { selectionStart: start, selectionEnd: end } = element;
    if (start !== null && end !== null && start !== end) return element.value.slice(start, end);
  }
  return doc.getSelection()?.toString() ?? null;
})()"#;

impl Engine {
    /// Inserts `text` at the focused element's caret, replacing its selection, as typed input.
    pub fn paste_text(&self, tab: &flecha_core::TabId, text: &str) -> bool {
        let Some(host) = self.host(tab) else {
            return false;
        };
        // CEF reads a missing range as the valid empty range at offset 0, and Chromium then
        // drops the commit. The invalid range means "no replacement" as the header documents.
        let unspecified = Range {
            from: u32::MAX,
            to: u32::MAX,
        };
        host.ime_commit_text(Some(&CefString::from(text)), Some(&unspecified), 0);
        true
    }

    /// Reads the page's selection for the shell's clipboard. The answer arrives from
    /// [`Self::next_copied_selection`]. With text, a [`ClipboardEdit::Cut`] then deletes the
    /// selection. Without, the page gets `unread` instead, such as the keystroke the shell
    /// held back, so a page that binds the key still sees it. Returns `false` when the read
    /// could not be sent; `unread` is dropped then.
    ///
    /// Chromium's own Copy and Cut never run here: on X11 they make Chromium the clipboard
    /// owner, and the shell's next read then waits on Chromium on the thread they share.
    pub fn copy_selection(
        &self,
        tab: &flecha_core::TabId,
        edit: ClipboardEdit,
        unread: impl FnOnce(&BrowserHost) + 'static,
    ) -> bool {
        let (Some(session), Some(host)) = (self.browsers.get(tab), self.host(tab)) else {
            return false;
        };
        let params = serde_json::json!({ "expression": READ_SELECTION, "returnByValue": true });
        let read = Selection {
            edit,
            unread: Box::new(unread),
        };
        devtools::send(
            &session.slot,
            &host,
            "Runtime.evaluate",
            Some(&params.to_string()),
            Some(devtools::Internal::Selection(read)),
        )
        .is_ok()
    }

    /// The next selection a [`Self::copy_selection`] call read, from a browser the engine still
    /// owns.
    pub fn next_copied_selection(&mut self) -> Option<CopiedSelection> {
        loop {
            let (tab, text) = self.services().copied.pop()?;
            if let Some(tab) = tab {
                return Some(CopiedSelection { tab, text });
            }
        }
    }
}

/// A pending [`Engine::copy_selection`].
pub(crate) struct Selection {
    edit: ClipboardEdit,
    unread: Box<dyn FnOnce(&BrowserHost)>,
}

/// The page answered [`Engine::copy_selection`].
pub(crate) fn read(
    slot: &std::rc::Rc<Slot>,
    browser: Option<&mut Browser>,
    read: Selection,
    reply: Option<&[u8]>,
) {
    let Some(browser) = browser.filter(|_| !slot.retired.get()) else {
        return;
    };
    let text = selection(reply);
    let copied = text.is_some();
    services::of(slot)
        .copied
        .push(std::rc::Rc::downgrade(slot), text);
    if !copied {
        if let Some(host) = browser.host() {
            (read.unread)(&host);
        }
    } else if read.edit == ClipboardEdit::Cut
        && let Some(frame) = browser.focused_frame()
    {
        frame.del();
    }
}

/// The selected text in a successful `Runtime.evaluate` reply to [`READ_SELECTION`]. An
/// exception, a failed call or an empty selection yields nothing.
fn selection(reply: Option<&[u8]>) -> Option<String> {
    let reply: serde_json::Value = serde_json::from_slice(reply?).ok()?;
    if reply.get("exceptionDetails").is_some() {
        return None;
    }
    let text = reply["result"]["value"].as_str()?;
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_selected_string_reaches_the_clipboard() {
        let reply = |value: serde_json::Value| {
            serde_json::json!({ "result": { "type": "string", "value": value } }).to_string()
        };
        assert_eq!(
            selection(Some(reply("selected words".into()).as_bytes())),
            Some("selected words".into())
        );
        for refused in [
            reply(serde_json::Value::Null),
            reply("".into()),
            reply(7.into()),
            serde_json::json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": { "text": "Uncaught" },
            })
            .to_string(),
            r#"{"code":-32000,"message":"Cannot find context"}"#.into(),
            "not json".into(),
        ] {
            assert_eq!(selection(Some(refused.as_bytes())), None, "{refused}");
        }
        assert_eq!(selection(None), None);
    }
}
