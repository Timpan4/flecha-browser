use std::ops::Range;

use gpui::{
    App, ClipboardItem, Div, FontWeight, Keystroke, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, relative,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::theme::palette;
use crate::tokens;
use crate::view::hairline;

#[derive(Default)]
pub(crate) struct TextField {
    text: String,
    cursor: usize,
    anchor: usize,
    multiline: bool,
    masked: bool,
}

impl TextField {
    pub(crate) fn new(text: String, select_all: bool) -> Self {
        let cursor = text.len();
        let anchor = if select_all { 0 } else { cursor };
        Self {
            text,
            cursor,
            anchor,
            multiline: false,
            masked: false,
        }
    }

    pub(crate) fn password() -> Self {
        Self {
            masked: true,
            ..Self::default()
        }
    }

    pub(crate) fn multiline(text: String, select_all: bool) -> Self {
        Self {
            multiline: true,
            ..Self::new(text, select_all)
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn at_end(&self) -> bool {
        self.cursor == self.text.len() && self.anchor == self.cursor
    }

    pub(crate) fn append_selected(&mut self, suffix: &str) {
        self.anchor = self.text.len();
        self.text.push_str(suffix);
        self.cursor = self.text.len();
    }

    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn replace(&mut self, text: &str) {
        let range = self.selection();
        let text = if self.multiline {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.replace(['\r', '\n'], " ")
        };
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
        self.text.replace_range(range, &text);
    }

    pub(crate) fn key(&mut self, stroke: &Keystroke, cx: &mut App) {
        let mods = stroke.modifiers;
        let primary = if cfg!(target_os = "macos") {
            mods.platform
        } else {
            mods.control
        };
        if primary && !mods.alt {
            match stroke.key.as_str() {
                "a" => {
                    self.anchor = 0;
                    self.cursor = self.text.len();
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.replace(&text);
                    }
                }
                "c" | "x" if !self.masked && self.anchor != self.cursor => {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.text[self.selection()].to_owned(),
                    ));
                    if stroke.key == "x" {
                        self.replace("");
                    }
                }
                _ => {}
            }
            return;
        }
        let previous = || {
            self.text
                .grapheme_indices(true)
                .rev()
                .find_map(|(index, _)| (index < self.cursor).then_some(index))
                .unwrap_or(0)
        };
        let next = || {
            self.text
                .grapheme_indices(true)
                .find_map(|(index, _)| (index > self.cursor).then_some(index))
                .unwrap_or(self.text.len())
        };
        let selection = self.selection();
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |line| line + 1);
        let line_end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |end| self.cursor + end);
        match stroke.key.as_str() {
            "left" | "right" | "home" | "end" => {
                self.cursor = match stroke.key.as_str() {
                    "left" if !mods.shift && !selection.is_empty() => selection.start,
                    "right" if !mods.shift && !selection.is_empty() => selection.end,
                    "left" => previous(),
                    "right" => next(),
                    "home" if self.multiline => line_start,
                    "home" => 0,
                    _ if self.multiline => line_end,
                    _ => self.text.len(),
                };
                if !mods.shift {
                    self.anchor = self.cursor;
                }
            }
            "up" | "down" if self.multiline => {
                let target = if stroke.key == "up" {
                    (line_start > 0).then(|| {
                        let end = line_start - 1;
                        let start = self.text[..end].rfind('\n').map_or(0, |line| line + 1);
                        start..end
                    })
                } else {
                    (line_end < self.text.len()).then(|| {
                        let start = line_end + 1;
                        let end = self.text[start..]
                            .find('\n')
                            .map_or(self.text.len(), |end| start + end);
                        start..end
                    })
                };
                if let Some(line) = target {
                    let column = self.text[line_start..self.cursor].graphemes(true).count();
                    self.cursor = self.text[line.clone()]
                        .grapheme_indices(true)
                        .nth(column)
                        .map_or(line.end, |(offset, _)| line.start + offset);
                    if !mods.shift {
                        self.anchor = self.cursor;
                    }
                }
            }
            "enter" if self.multiline => self.replace("\n"),
            "tab" if self.multiline => self.replace("\t"),
            "backspace" | "delete" => {
                if selection.is_empty() {
                    self.anchor = if stroke.key == "backspace" {
                        previous()
                    } else {
                        next()
                    };
                }
                self.replace("");
            }
            _ if let Some(text) = &stroke.key_char => self.replace(text),
            _ => {}
        }
    }

    pub(crate) fn render(&self, placeholder: &'static str) -> Div {
        if self.multiline {
            // `.env-targets` (ui/settings.css:405-417).
            let mut offset = 0;
            return div()
                .flex()
                .flex_col()
                .gap_0()
                .min_w_0()
                .overflow_hidden()
                .px(px(tokens::MULTILINE_PAD_X))
                .py(px(tokens::MULTILINE_PAD_Y))
                .rounded(px(tokens::RADIUS_MD))
                .bg(palette().surface_2)
                .font_family(tokens::MONO)
                .text_size(px(tokens::MULTILINE_TEXT))
                .line_height(relative(tokens::MULTILINE_LINE_HEIGHT))
                .children(self.text.split('\n').map(|line| {
                    let end = offset + line.len();
                    let rendered = self.render_line(offset..end);
                    offset = end + 1;
                    rendered
                }))
                .when(self.text.is_empty(), |field| {
                    field.child(div().text_color(palette().text_3).child(placeholder))
                });
        }
        // `.text-field` (ui/styles.css:1434-1448).
        let field = div()
            .flex()
            .items_center()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .h(px(tokens::TEXT_FIELD))
            .px(px(tokens::TEXT_FIELD_PAD))
            .rounded(px(tokens::RADIUS_MD))
            .bg(palette().surface_2)
            .shadow(vec![hairline(palette().surface_line)])
            .font_weight(FontWeight::MEDIUM);
        if self.text.is_empty() {
            return field
                .child(caret())
                .child(div().ml_1().text_color(palette().text_3).child(placeholder));
        }
        field.child(self.render_line(0..self.text.len()))
    }

    /// The text alone, for a field whose box its owner draws, such as Settings' search or
    /// Flechinha's address (ui/flechinha.css:224-238): the caret and selection show only while
    /// `focused`.
    pub(crate) fn render_inline(&self, placeholder: &'static str, focused: bool) -> Div {
        let field = div()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap();
        match (self.text.is_empty(), focused) {
            (true, true) => field.child(caret()).child(placeholder),
            (true, false) => field.child(placeholder),
            (false, true) => field
                .text_color(palette().text)
                .child(self.render_line(0..self.text.len())),
            (false, false) => field.text_color(palette().text).child(self.text.clone()),
        }
    }

    fn render_line(&self, line: Range<usize>) -> Div {
        let selection = self.selection();
        let start = selection.start.clamp(line.start, line.end);
        let end = selection.end.clamp(line.start, line.end);
        let has_caret = line.contains(&self.cursor) || self.cursor == line.end;
        let display = |range| {
            let text: &str = &self.text[range];
            if self.masked {
                "•".repeat(text.graphemes(true).count())
            } else {
                text.to_owned()
            }
        };
        let selected = div()
            .flex_shrink_0()
            .bg(palette().focus)
            .child(display(start..end));
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .min_h(px(16.))
            .whitespace_nowrap()
            .child(display(line.start..start))
            .when(has_caret && self.cursor == start, |line| {
                line.child(caret())
            })
            .child(selected)
            .when(has_caret && self.cursor != start, |line| {
                line.child(caret())
            })
            .child(display(end..line.end))
    }
}

/// The insertion point, in the text's colour as Chromium draws an unstyled input's caret.
fn caret() -> Div {
    div()
        .w(px(1.))
        .h(px(16.))
        .flex_shrink_0()
        .bg(palette().text)
}
