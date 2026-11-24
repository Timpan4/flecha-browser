//! Native text input for CEF's off-screen browser, with UTF-16 offsets throughout.

use std::cell::RefCell;
use std::ops::Range;

use cef::{BrowserHost, CefString, ImplBrowser as _, ImplBrowserHost as _, Rect};
use gpui::{Bounds, Pixels, Point, UTF16Selection, point, px, size};

use crate::callbacks::Slot;
use crate::{BrowserKey, Engine};

pub enum ImeEdit {
    Mark {
        text: String,
        replacement: Option<Range<usize>>,
        selection: Option<Range<usize>>,
    },
    Commit {
        text: String,
        replacement: Option<Range<usize>>,
    },
    Unmark,
    Cancel,
}

#[derive(Default)]
pub struct Ime(RefCell<State>);

#[derive(Default)]
struct State {
    focused: bool,
    selection: Option<UTF16Selection>,
    marked: Option<Marked>,
}

struct Marked {
    start: usize,
    text: Vec<u16>,
    selection: Range<usize>,
    bounds: Vec<Bounds<Pixels>>,
}

impl Marked {
    fn range(&self) -> Range<usize> {
        self.start..self.start.saturating_add(self.text.len())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Mark {
        text: String,
        replacement: Option<Range<usize>>,
        selection: Range<usize>,
    },
    Commit {
        text: String,
        replacement: Option<Range<usize>>,
    },
    Finish,
    Cancel,
}

impl State {
    fn focus(&mut self, focused: bool) -> Option<Command> {
        self.focused = focused;
        if focused {
            return None;
        }
        self.marked.take().map(|_| Command::Cancel)
    }

    fn edit(&mut self, edit: ImeEdit) -> Option<Command> {
        if !self.focused {
            return None;
        }
        match edit {
            ImeEdit::Mark {
                text,
                replacement,
                selection,
            } if !text.is_empty() => {
                let utf16: Vec<_> = text.encode_utf16().collect();
                let len = utf16.len();
                let selection =
                    selection.map_or(len..len, |range| range.start.min(len)..range.end.min(len));
                let start = replacement
                    .as_ref()
                    .map(|range| range.start)
                    .or_else(|| self.marked.as_ref().map(|marked| marked.start))
                    .or_else(|| {
                        self.selection
                            .as_ref()
                            .map(|selection| selection.range.start)
                    })
                    .unwrap_or_default();
                self.marked = Some(Marked {
                    start,
                    text: utf16,
                    selection: selection.clone(),
                    bounds: Vec::new(),
                });
                Some(Command::Mark {
                    text,
                    replacement,
                    selection,
                })
            }
            ImeEdit::Commit { text, replacement } => {
                let marked = self.marked.take();
                let replaces_marked = marked.is_some_and(|marked| {
                    replacement
                        .as_ref()
                        .is_some_and(|range| *range == marked.range())
                });
                if replaces_marked && text.is_empty() {
                    Some(Command::Cancel)
                } else {
                    Some(Command::Commit {
                        text,
                        replacement: replacement.filter(|_| !replaces_marked),
                    })
                }
            }
            ImeEdit::Unmark => self.marked.take().map(|_| Command::Finish),
            ImeEdit::Mark { .. } | ImeEdit::Cancel => self.marked.take().map(|_| Command::Cancel),
        }
    }
}

impl Ime {
    pub fn marked_range(&self) -> Option<Range<usize>> {
        Some(self.0.borrow().marked.as_ref()?.range())
    }

    pub fn selection(&self) -> Option<UTF16Selection> {
        let state = self.0.borrow();
        if let Some(marked) = &state.marked {
            let start = marked.start.saturating_add(marked.selection.start);
            let end = marked.start.saturating_add(marked.selection.end);
            return Some(UTF16Selection {
                range: start.min(end)..start.max(end),
                reversed: end < start,
            });
        }
        state.selection.as_ref().map(|selection| UTF16Selection {
            range: selection.range.clone(),
            reversed: selection.reversed,
        })
    }

    pub fn marked_text(&self, range: Range<usize>) -> Option<String> {
        let state = self.0.borrow();
        let marked = state.marked.as_ref()?;
        let start = range.start.checked_sub(marked.start)?;
        let end = range.end.checked_sub(marked.start)?;
        String::from_utf16(marked.text.get(start..end)?).ok()
    }

    pub fn bounds_for_range(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let state = self.0.borrow();
        let marked = state.marked.as_ref()?;
        let start = range.start.checked_sub(marked.start)?;
        if range.is_empty() && start == marked.bounds.len() {
            let last = marked.bounds.last()?;
            return Some(Bounds::new(
                point(last.origin.x + last.size.width, last.origin.y),
                size(px(0.), last.size.height),
            ));
        }
        let first = *marked.bounds.get(start)?;
        if range.is_empty() {
            return Some(Bounds::new(first.origin, size(px(0.), first.size.height)));
        }
        let end = range.end.checked_sub(marked.start)?;
        Some(
            marked
                .bounds
                .get(start..end)?
                .iter()
                .fold(first, |bounds, next| bounds.union(next)),
        )
    }

    pub fn character_index(&self, position: Point<Pixels>) -> Option<usize> {
        let state = self.0.borrow();
        let marked = state.marked.as_ref()?;
        marked
            .bounds
            .iter()
            .position(|bounds| bounds.contains(&position))?
            .checked_add(marked.start)
    }

    pub(crate) fn update_bounds(&self, range: Option<&cef::Range>, rects: &[Rect]) {
        let Some(range) = range.and_then(from_cef) else {
            return;
        };
        let mut state = self.0.borrow_mut();
        let Some(marked) = &mut state.marked else {
            return;
        };
        if range.len() == marked.text.len() {
            marked.start = range.start;
            if rects.len() == range.len() {
                marked.bounds = rects
                    .iter()
                    .map(|rect| {
                        Bounds::new(
                            point(px(rect.x as f32), px(rect.y as f32)),
                            size(px(rect.width as f32), px(rect.height as f32)),
                        )
                    })
                    .collect();
            }
        }
    }

    pub(crate) fn update_selection(&self, range: Option<&cef::Range>) {
        self.0.borrow_mut().selection = range.and_then(from_cef).map(|range| UTF16Selection {
            reversed: range.start > range.end,
            range: range.start.min(range.end)..range.start.max(range.end),
        });
    }

    pub(crate) fn reset_document(&self) {
        let mut state = self.0.borrow_mut();
        state.marked = None;
        state.selection = None;
    }
}

fn from_cef(range: &cef::Range) -> Option<Range<usize>> {
    (range.from != u32::MAX && range.to != u32::MAX)
        .then_some(range.from as usize..range.to as usize)
}

fn to_cef(range: Option<Range<usize>>) -> cef::Range {
    // CEF requires a range pointer. gfx::Range::InvalidRange() uses u32::MAX for both ends.
    range
        .and_then(|range| {
            Some(cef::Range {
                from: range.start.try_into().ok()?,
                to: range.end.try_into().ok()?,
            })
        })
        .unwrap_or(cef::Range {
            from: u32::MAX,
            to: u32::MAX,
        })
}

impl Command {
    fn send(self, host: &BrowserHost) {
        match self {
            Self::Mark {
                text,
                replacement,
                selection,
            } => host.ime_set_composition(
                Some(&CefString::from(text.as_str())),
                None,
                Some(&to_cef(replacement)),
                Some(&to_cef(Some(selection))),
            ),
            Self::Commit { text, replacement } => host.ime_commit_text(
                Some(&CefString::from(text.as_str())),
                Some(&to_cef(replacement)),
                0,
            ),
            Self::Finish => host.ime_finish_composing_text(1),
            Self::Cancel => host.ime_cancel_composition(),
        }
    }
}

impl Slot {
    pub(crate) fn set_page_focus(&self, focused: bool) {
        if self.ime.0.borrow().focused == focused {
            return;
        }
        let Some(host) = self
            .browser
            .borrow()
            .as_ref()
            .and_then(|browser| browser.host())
        else {
            return;
        };
        let command = self.ime.0.borrow_mut().focus(focused);
        if let Some(command) = command {
            command.send(&host);
        }
        host.set_focus(i32::from(focused));
    }
}

impl Engine {
    /// IME state for the focused browser generation.
    pub fn ime(&self, key: &BrowserKey) -> Option<&Ime> {
        let slot = &self.browsers.get(&key.tab)?.slot;
        (self.generations.accepts(key) && slot.ime.0.borrow().focused).then_some(&slot.ime)
    }

    pub fn ime_edit(&self, key: &BrowserKey, edit: ImeEdit) {
        let Some(ime) = self.ime(key) else { return };
        let Some(host) = self.host(&key.tab) else {
            return;
        };
        let command = ime.0.borrow_mut().edit(edit);
        if let Some(command) = command {
            command.send(&host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(text: &str) -> ImeEdit {
        ImeEdit::Mark {
            text: text.into(),
            replacement: None,
            selection: None,
        }
    }

    fn commit(text: &str) -> ImeEdit {
        ImeEdit::Commit {
            text: text.into(),
            replacement: None,
        }
    }

    #[test]
    fn updates_utf16_preedit_and_commits_once() {
        let ime = Ime::default();
        ime.0.borrow_mut().focus(true);
        ime.update_selection(Some(&cef::Range { from: 4, to: 4 }));
        assert_eq!(
            ime.0.borrow_mut().edit(mark("é😀")),
            Some(Command::Mark {
                text: "é😀".into(),
                replacement: None,
                selection: 3..3,
            })
        );
        assert_eq!(ime.marked_range(), Some(4..7));
        assert_eq!(ime.marked_text(5..7).as_deref(), Some("😀"));
        ime.0.borrow_mut().edit(mark("漢字"));
        assert_eq!(ime.selection().unwrap().range, 6..6);
        assert_eq!(
            ime.0.borrow_mut().edit(commit("漢字")),
            Some(Command::Commit {
                text: "漢字".into(),
                replacement: None,
            })
        );
        assert_eq!(ime.marked_range(), None);
        assert_eq!(ime.0.borrow_mut().edit(ImeEdit::Unmark), None);
    }

    #[test]
    fn unmark_finishes_and_empty_preedit_cancels() {
        let mut state = State::default();
        state.focus(true);
        state.edit(mark("仮"));
        assert_eq!(state.edit(ImeEdit::Unmark), Some(Command::Finish));
        state.edit(mark("仮"));
        assert_eq!(state.edit(mark("")), Some(Command::Cancel));
        assert_eq!(state.edit(mark("")), None);
    }

    #[test]
    fn blur_and_tab_switch_cancel_the_old_browser_composition() {
        let (mut old, mut next) = (State::default(), State::default());
        old.focus(true);
        old.edit(mark("´"));
        assert_eq!(old.focus(false), Some(Command::Cancel));
        next.focus(true);
        assert_eq!(old.edit(commit("é")), None);
        assert!(next.marked.is_none());
        next.edit(mark("い"));
        assert_eq!(next.focus(false), Some(Command::Cancel));
        old.focus(true);
        assert!(old.marked.is_none());
        assert_eq!(old.focus(false), None);
    }
}
