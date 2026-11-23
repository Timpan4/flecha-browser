use std::collections::HashMap;
use std::ops::Range;

use serde_json::{Map, Value, value::RawValue};

use super::snapshot::{lossy_string, scalar_text};

// At serde_json's default recursion limit, a Space and its envelope consume two
// containers. Each folder consumes an object and child array. Reserve two more
// containers for the final list and tab, whose enclosing depth must stay below 128.
const FOLDER_LEVELS: usize = (128 - 1 - 2 - 2) / 2;

pub(super) fn project(raw: &RawValue, flattened: &mut usize) -> serde_json::Result<Value> {
    let text = raw.get();
    if !text.starts_with('[') {
        return Ok(Value::Null);
    }

    let layout = JsonLayout::new(text);
    let mut pending = vec![(1..text.len() - 1, 0)];
    let mut pins = Vec::new();
    while let Some((entries, depth)) = pending.last_mut() {
        let Some(node) = layout.next(entries) else {
            pending.pop();
            continue;
        };
        if layout.byte(node.start) != b'{' {
            continue;
        }

        let mut fields = Map::new();
        let mut children = None;
        let mut entries = inside(node);
        while let Some(key) = layout.next(&mut entries) {
            let value = layout
                .next(&mut entries)
                .expect("validated JSON object has a value");
            match lossy_string(layout.text(key))?.as_str() {
                "children" => children = Some(value),
                key @ ("id" | "kind" | "tabId" | "name" | "open") => {
                    fields.insert(key.into(), scalar_text(layout.text(value)));
                }
                _ => {}
            }
        }
        let depth = *depth;
        let folder = fields.get("kind").and_then(Value::as_str) == Some("folder")
            && fields.get("id").is_some_and(Value::is_string);
        let tab = fields.get("kind").and_then(Value::as_str) == Some("tab");
        if !folder && !tab {
            continue;
        }
        let retain = !folder || depth < FOLDER_LEVELS;
        if retain {
            fields.insert("depth".into(), Value::from(depth));
            pins.push(Value::Object(fields));
        } else {
            *flattened += 1;
        }
        if folder && let Some(children) = children.filter(|range| layout.byte(range.start) == b'[')
        {
            pending.push((inside(children), depth + usize::from(retain)));
        }
    }
    Ok(Value::Array(pins))
}

fn inside(container: Range<usize>) -> Range<usize> {
    container.start + 1..container.end - 1
}

/// Offsets in a validated RawValue. Index containers once so reading the fields
/// of every folder does not repeatedly scan its descendants.
struct JsonLayout<'a> {
    text: &'a str,
    ends: HashMap<usize, usize>,
}

impl<'a> JsonLayout<'a> {
    fn new(text: &'a str) -> Self {
        let mut ends = HashMap::new();
        let mut open = Vec::new();
        let mut cursor = 0;
        while cursor < text.len() {
            match text.as_bytes()[cursor] {
                b'"' => {
                    cursor = string_end(text.as_bytes(), cursor);
                    continue;
                }
                b'{' | b'[' => open.push(cursor),
                b'}' | b']' => {
                    let start = open.pop().expect("validated JSON balances containers");
                    ends.insert(start, cursor + 1);
                }
                _ => {}
            }
            cursor += 1;
        }
        Self { text, ends }
    }

    fn next(&self, input: &mut Range<usize>) -> Option<Range<usize>> {
        let bytes = self.text.as_bytes();
        while input.start < input.end
            && matches!(
                bytes[input.start],
                b',' | b':' | b' ' | b'\t' | b'\r' | b'\n'
            )
        {
            input.start += 1;
        }
        let start = input.start;
        if start == input.end {
            return None;
        }
        let end = match bytes[start] {
            b'{' | b'[' => self.ends[&start],
            b'"' => string_end(bytes, start),
            _ => bytes[start..input.end]
                .iter()
                .position(|&ch| matches!(ch, b',' | b':' | b' ' | b'\t' | b'\r' | b'\n'))
                .map_or(input.end, |offset| start + offset),
        };
        input.start = end;
        Some(start..end)
    }

    fn byte(&self, start: usize) -> u8 {
        self.text.as_bytes()[start]
    }

    fn text(&self, range: Range<usize>) -> &str {
        &self.text[range]
    }
}

fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start + 1;
    loop {
        match bytes[cursor] {
            b'"' => return cursor + 1,
            b'\\' => cursor += 2,
            _ => cursor += 1,
        }
    }
}
