use cap_std::fs::Dir;
use serde_json::{Map, Value, value::RawValue};

use super::{file, read_text, repair_surrogates};
use crate::ReadError;

// Chromium's base/json/json_common.h defines a JSON depth of 200. Allow that many
// bookmark folders, each an object plus a children array, and the four wrappers
// around the folder chain plus the leaf bookmark object.
const CHROMIUM_JSON_DEPTH: usize = 200;
const MAX_JSON_CONTAINERS: usize = 2 * CHROMIUM_JSON_DEPTH + 5;

pub(crate) struct JsonRead {
    value: Value,
    out_of_range: usize,
    too_deep: usize,
}

impl JsonRead {
    pub(crate) fn into_value(self, label: &str, warnings: &mut Vec<String>) -> Value {
        if self.out_of_range > 0 {
            warnings.push(format!(
                "Replaced {} out-of-range JSON numbers with null in {label}.",
                self.out_of_range
            ));
        }
        if self.too_deep > 0 {
            warnings.push(format!(
                "Skipped {} JSON subtrees beyond the nesting limit in {label}.",
                self.too_deep
            ));
        }
        self.value
    }
}

pub(crate) fn json(dir: &Dir, name: &str) -> Result<JsonRead, ReadError> {
    parse(&read_text(file(dir, name)?)?)
}

fn parse(text: &str) -> Result<JsonRead, ReadError> {
    // RawValue validates the entire document with serde_json's iterative scanner.
    // The projection below decides which containers and scalar values to keep.
    let raw: &RawValue = serde_json::from_str(text).map_err(|_| ReadError::InvalidJson)?;
    let mut tokens = Tokens(raw.get());
    let mut frames = Vec::new();
    let mut out_of_range = 0;
    let mut too_deep = 0;
    while let Some(token) = tokens.next() {
        let value = match token {
            "[" | "{" if frames.len() == MAX_JSON_CONTAINERS => {
                tokens.skip_collection();
                too_deep += 1;
                Value::Null
            }
            "{" => {
                frames.push(Frame::Object(Map::new(), None));
                continue;
            }
            "[" => {
                frames.push(Frame::Array(Vec::new()));
                continue;
            }
            "}" | "]" => match frames.pop().expect("validated closing delimiter") {
                Frame::Object(fields, _) => Value::Object(fields),
                Frame::Array(values) => Value::Array(values),
            },
            "," | ":" => continue,
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "null" => Value::Null,
            text if text.starts_with('"') => {
                let value = serde_json::from_str(&repair_surrogates(text))
                    .map_err(|_| ReadError::InvalidJson)?;
                Value::String(value)
            }
            number => match serde_json::from_str(number) {
                Ok(value) => value,
                Err(_) => {
                    // The token's syntax was already validated, including its exponent.
                    out_of_range += 1;
                    Value::Null
                }
            },
        };
        match frames.last_mut() {
            Some(Frame::Object(fields, key)) => {
                if let Some(key) = key.take() {
                    fields.insert(key, value);
                } else if let Value::String(name) = value {
                    *key = Some(name);
                } else {
                    unreachable!("validated object key");
                }
            }
            Some(Frame::Array(values)) => values.push(value),
            None => {
                return Ok(JsonRead {
                    value,
                    out_of_range,
                    too_deep,
                });
            }
        }
    }
    Err(ReadError::InvalidJson)
}

enum Frame {
    Object(Map<String, Value>, Option<String>),
    Array(Vec<Value>),
}

struct Tokens<'a>(&'a str);

impl Tokens<'_> {
    fn skip_collection(&mut self) {
        let mut depth = 1;
        for token in self {
            match token {
                "[" | "{" => depth += 1,
                "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
        unreachable!("validated collection");
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let input = self.0.trim_start_matches([' ', '\n', '\r', '\t']);
        let bytes = input.as_bytes();
        let end = match *bytes.first()? {
            b'"' => {
                let mut end = 1;
                loop {
                    match bytes[end] {
                        b'"' => break end + 1,
                        b'\\' => end += 2,
                        _ => end += 1,
                    }
                }
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => 1,
            _ => bytes
                .iter()
                .position(|byte| matches!(byte, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t'))
                .unwrap_or(bytes.len()),
        };
        let (token, rest) = input.split_at(end);
        self.0 = rest;
        Some(token)
    }
}
