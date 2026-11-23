use std::io::{self, Cursor, Write};

use cap_std::fs::Dir;
use plist::Value;
use plist::stream::{Event, Reader, Writer, XmlWriter};

use crate::input::{file, read_bytes};
use crate::{Bookmark, ReadError, SavedTab};

// The shipping macOS reader allows this much XML from plutil, including expansion of binary references.
const MAX_PLIST_BYTES: u64 = 64 * 1024 * 1024;
// Match serde_json's default nesting boundary when materializing a plist value tree.
const MAX_COLLECTION_DEPTH: usize = 127;

pub(crate) fn bookmarks(root: &Dir) -> Result<(Vec<Bookmark>, usize), ReadError> {
    let data = read_bytes(file(root, "Bookmarks.plist")?, MAX_PLIST_BYTES)?;
    let mut limit = XmlBudget(MAX_PLIST_BYTES);
    let mut writer = XmlWriter::new(&mut limit);
    let mut depth: usize = 0;
    let events = Reader::new(Cursor::new(data))
        .take_while(|event| {
            match event {
                Ok(Event::StartArray(_) | Event::StartDictionary(_)) => depth += 1,
                Ok(Event::EndCollection) => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth <= MAX_COLLECTION_DEPTH
        })
        .map(|event| {
            let event = event?;
            writer.write(event.clone())?;
            Ok(event)
        });
    let plist = Value::from_events(events).map_err(|_| ReadError::InvalidPropertyList)?;
    let mut malformed = 0;
    let mut result = Vec::new();
    if let Some(children) = items(&plist) {
        for child in children {
            if field(child, "Title").and_then(Value::as_string) == Some("BookmarksBar") {
                if let Some(children) = items(child) {
                    result.extend(nodes(children, &mut malformed));
                }
            } else {
                result.extend(nodes(std::slice::from_ref(child), &mut malformed));
            }
        }
    }
    Ok((result, malformed))
}

fn nodes(items: &[Value], malformed: &mut usize) -> Vec<Bookmark> {
    items
        .iter()
        .filter_map(
            |item| match field(item, "WebBookmarkType").and_then(Value::as_string) {
                Some("WebBookmarkTypeLeaf") => {
                    let Some(url) = field(item, "URLString").and_then(Value::as_string) else {
                        *malformed += 1;
                        return None;
                    };
                    let title = field(item, "URIDictionary").and_then(|dict| field(dict, "title"));
                    Some(Bookmark::Tab(SavedTab {
                        url: url.into(),
                        title: plist_text(title).unwrap_or_else(|| url.into()),
                    }))
                }
                Some("WebBookmarkTypeList") => {
                    let title = plist_text(field(item, "Title"))
                        .unwrap_or_else(|| "Untitled folder".into());
                    let name = match title.as_str() {
                        "BookmarksMenu" => "Bookmarks Menu".into(),
                        "com.apple.ReadingList" => "Reading List".into(),
                        _ => title,
                    };
                    let children =
                        self::items(item).map_or_else(Vec::new, |items| nodes(items, malformed));
                    Some(Bookmark::Folder { name, children })
                }
                _ => None,
            },
        )
        .collect()
}

fn items(value: &Value) -> Option<&[Value]> {
    field(value, "Children")?.as_array().map(Vec::as_slice)
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_dictionary()?.get(key)
}

fn plist_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Integer(value) => Some(value.to_string()),
        Value::Real(value) => Some(value.to_string()),
        Value::Boolean(true) => Some("true".into()),
        _ => None,
    }
}

struct XmlBudget(u64);

impl Write for XmlBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len() as u64)
            .ok_or(io::ErrorKind::FileTooLarge)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
