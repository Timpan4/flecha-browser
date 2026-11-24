//! Page clipboard edits through the shell's clipboard.
//!
//! Under offscreen rendering Chromium has no window with keyboard focus, and on Wayland the
//! compositor serves clipboard reads and writes only to the focused client. Chromium's own
//! Copy, Cut and Paste therefore never reach the desktop clipboard there. The shell owns the
//! focused window, so it reads and writes the clipboard and the engine moves the text in and
//! out of the page: pasted text is committed as typed input, and copied text is read from the
//! page's selection through an internal CDP call.

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

/// Reads the selection of the page's focused element, and for a cut deletes it in the same
/// evaluation, so no key, timer or focus change can come between the read and the delete. It
/// answers `null`, so nothing reaches the clipboard, for a password field and for a focused
/// frame it cannot read, such as a cross-origin iframe, which could hold one. The delete acts
/// only in the document the text came from. A cut of text that cannot be deleted, such as a
/// read-only field or page text, answers `null` as Chromium cuts nothing there. When a page
/// handler changes what the delete removes, the answer is the text that left the element.
const READ_SELECTION: &str = r#"(cut) => {
  let doc = document;
  let element = doc.activeElement;
  for (;;) {
    while (element?.shadowRoot?.activeElement) element = element.shadowRoot.activeElement;
    if (!['iframe', 'frame', 'object', 'embed'].includes(element?.localName)) break;
    try {
      doc = element.contentDocument;
    } catch {
      return null;
    }
    if (!doc) return null;
    element = doc.activeElement;
  }
  // Only a text control holds the selection when focused. Any other focused element, such as
  // a checkbox, leaves it where it was, which may be a blurred password field.
  const typed = ['text', 'search', 'url', 'tel', 'email', 'number', 'password'];
  const field = element?.localName === 'textarea' || (element?.localName === 'input' && typed.includes(element.type));
  if (field && element.type === 'password') return null;
  const selection = doc.getSelection();
  let range = null;
  let text = null;
  if (field) {
    const { selectionStart: start, selectionEnd: end } = element;
    if (start !== null && end !== null && start !== end) {
      range = [start, end];
      text = element.value.slice(start, end);
    }
  }
  // A collapsed selection with text lies inside a text control. Unless it is the focused field
  // checked above, it may be a password field's: blurred, or in a closed shadow root.
  if (text === null && selection && !(selection.isCollapsed && !field)) text = selection.toString();
  if (!text || !cut) return text || null;
  // Delete only the text just read: a selection in the field it came from, or in the focused
  // editing host. Delete a field's selection only when it is not collapsed, or the command
  // would delete the character before the caret. Like Chromium, cut nothing it cannot delete.
  const host = field ? element : element?.isContentEditable ? element : null;
  const selected = field ? range !== null || element.selectionStart === null : !selection.isCollapsed;
  if (!host || !selected || (field && element.readOnly)) return null;
  const property = field ? 'value' : 'textContent';
  const before = host[property];
  const expected = range ? before.slice(0, range[0]) + before.slice(range[1]) : null;
  const length = field ? text.length : selection.getRangeAt(0).cloneContents().textContent.length;
  doc.execCommand('delete');
  const after = host[property];
  if (after === expected || (expected === null && before.length - after.length === length)) return text;
  // A page handler changed the edit. Copy what left the element, or the selection when nothing did.
  let start = 0;
  while (start < Math.min(before.length, after.length) && before[start] === after[start]) start++;
  let end = 0;
  while (end < Math.min(before.length, after.length) - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  return before.slice(start, before.length - end) || text;
}"#;

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
    /// [`Self::next_copied_selection`]. A [`ClipboardEdit::Cut`] deletes what it read in the
    /// same step. Without text, the page gets `unread` instead, such as the keystroke the shell
    /// held back, so a page that binds the key still sees it. Returns `false` when the read
    /// could not be sent; `unread` is dropped then.
    ///
    /// Chromium's own Copy and Cut never run here: were Chromium the clipboard owner, the
    /// shell's next read would wait on Chromium on the thread they share.
    pub fn copy_selection(
        &self,
        tab: &flecha_core::TabId,
        edit: ClipboardEdit,
        unread: impl FnOnce(&BrowserHost) + 'static,
    ) -> bool {
        let (Some(session), Some(host)) = (self.browsers.get(tab), self.host(tab)) else {
            return false;
        };
        let cut = edit == ClipboardEdit::Cut;
        let expression = format!("({READ_SELECTION})({cut})");
        let params = serde_json::json!({ "expression": expression, "returnByValue": true });
        let read = Selection {
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
    unread: Box<dyn FnOnce(&BrowserHost)>,
}

/// The page answered [`Engine::copy_selection`].
pub(crate) fn read(
    slot: &std::rc::Rc<Slot>,
    browser: Option<&mut Browser>,
    read: Selection,
    reply: Option<&[u8]>,
) {
    // Every read is answered, so the shell stops holding the browser's later input.
    let text = selection(reply);
    let copied = text.is_some();
    services::of(slot)
        .copied
        .push(std::rc::Rc::downgrade(slot), text);
    if !copied
        && !slot.retired.get()
        && let Some(host) = browser.and_then(|browser| browser.host())
    {
        (read.unread)(&host);
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
