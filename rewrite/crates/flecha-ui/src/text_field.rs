use std::ops::Range;

use gpui::{
    App, ClipboardItem, Div, Keystroke, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _, px, rgb,
};
use unicode_segmentation::UnicodeSegmentation as _;

#[derive(Default)]
pub(crate) struct TextField {
    text: String,
    cursor: usize,
    anchor: usize,
    multiline: bool,
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
                "c" | "x" if self.anchor != self.cursor => {
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
            let mut offset = 0;
            return div()
                .flex()
                .flex_col()
                .gap_0()
                .min_w_0()
                .overflow_hidden()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(rgb(0x243640))
                .font_family("monospace")
                .children(self.text.split('\n').map(|line| {
                    let end = offset + line.len();
                    let rendered = self.render_line(offset..end);
                    offset = end + 1;
                    rendered
                }))
                .when(self.text.is_empty(), |field| {
                    field.child(div().text_color(rgb(0xa4b5c3)).child(placeholder))
                });
        }
        let field = div()
            .flex()
            .items_center()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgb(0x243640));
        if self.text.is_empty() {
            return field
                .child(caret())
                .child(div().ml_1().text_color(rgb(0xa4b5c3)).child(placeholder));
        }
        field.child(self.render_line(0..self.text.len()))
    }

    fn render_line(&self, line: Range<usize>) -> Div {
        let selection = self.selection();
        let start = selection.start.clamp(line.start, line.end);
        let end = selection.end.clamp(line.start, line.end);
        let has_caret = line.contains(&self.cursor) || self.cursor == line.end;
        let selected = div()
            .flex_shrink_0()
            .bg(rgb(0x285783))
            .child(self.text[start..end].to_owned());
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .min_h(px(16.))
            .whitespace_nowrap()
            .child(self.text[line.start..start].to_owned())
            .when(has_caret && self.cursor == start, |line| {
                line.child(caret())
            })
            .child(selected)
            .when(has_caret && self.cursor != start, |line| {
                line.child(caret())
            })
            .child(self.text[end..line.end].to_owned())
    }
}

fn caret() -> Div {
    div().w(px(1.)).h(px(16.)).flex_shrink_0().bg(rgb(0x80c8ee))
}
