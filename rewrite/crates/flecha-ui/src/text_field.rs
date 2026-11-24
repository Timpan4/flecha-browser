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
}

impl TextField {
    pub(crate) fn new(text: String, select_all: bool) -> Self {
        let cursor = text.len();
        let anchor = if select_all { 0 } else { cursor };
        Self {
            text,
            cursor,
            anchor,
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
        let text = text.replace(['\r', '\n'], " ");
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
        match stroke.key.as_str() {
            "left" | "right" | "home" | "end" => {
                self.cursor = match stroke.key.as_str() {
                    "left" if !mods.shift && !selection.is_empty() => selection.start,
                    "right" if !mods.shift && !selection.is_empty() => selection.end,
                    "left" => previous(),
                    "right" => next(),
                    "home" => 0,
                    _ => self.text.len(),
                };
                if !mods.shift {
                    self.anchor = self.cursor;
                }
            }
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
        let selection = self.selection();
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
        let selected = div()
            .flex_shrink_0()
            .bg(rgb(0x285783))
            .child(self.text[selection.clone()].to_owned());
        field.child(
            div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .child(self.text[..selection.start].to_owned())
                .when(self.cursor == selection.start, |line| line.child(caret()))
                .child(selected)
                .when(self.cursor != selection.start, |line| line.child(caret()))
                .child(self.text[selection.end..].to_owned()),
        )
    }
}

fn caret() -> Div {
    div().w(px(1.)).h(px(16.)).flex_shrink_0().bg(rgb(0x80c8ee))
}
