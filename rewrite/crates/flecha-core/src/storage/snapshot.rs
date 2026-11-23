use serde::Deserialize;
use serde::de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, value::RawValue};

use super::{DecodeError, RawRecord, SnapshotRepairs, legacy_pins};

#[derive(Clone, Copy)]
enum Shape {
    Scalar,
    Array(&'static Shape),
    Record(&'static [(&'static str, Shape)]),
    Dictionary(&'static Shape),
    LegacyPins,
}

use Shape::{Array, Dictionary, LegacyPins, Record, Scalar};

const TAB_IDS: Shape = Array(&Scalar);
const FLAT_PIN: Shape = Record(&[
    ("id", Scalar),
    ("kind", Scalar),
    ("tabId", Scalar),
    ("name", Scalar),
    ("open", Scalar),
    ("depth", Scalar),
]);

const WINDOW_SPACE: &[(&str, Shape)] = &[
    ("today", TAB_IDS),
    ("activeTabId", Scalar),
    ("split", TAB_IDS),
    ("splitMode", Scalar),
    ("splitRatio", Scalar),
];

const SPACE: Shape = Record(&[
    ("id", Scalar),
    ("name", Scalar),
    ("emoji", Scalar),
    ("color", Scalar),
    ("isolated", Scalar),
    ("pinnedCollapsed", Scalar),
    ("pinned", LegacyPins),
    ("pinnedFlat", Array(&FLAT_PIN)),
    // Version 1 stored the window's Space membership on the Space itself.
    ("today", TAB_IDS),
    ("activeTabId", Scalar),
    ("split", TAB_IDS),
    ("splitMode", Scalar),
    ("splitRatio", Scalar),
]);

const WINDOW: Shape = Record(&[
    ("id", Scalar),
    ("activeSpaceId", Scalar),
    ("sidebarVisible", Scalar),
    ("closedAt", Scalar),
    ("spaces", Dictionary(&Record(WINDOW_SPACE))),
]);

const ARCHIVE_ENTRY: Shape = Record(&[("url", Scalar), ("title", Scalar), ("closedAt", Scalar)]);
const ENVIRONMENT_TARGET: Shape =
    Record(&[("label", Scalar), ("origin", Scalar), ("autoLabel", Scalar)]);
const ENVIRONMENT: Shape = Record(&[
    ("id", Scalar),
    ("name", Scalar),
    ("targets", Array(&ENVIRONMENT_TARGET)),
]);
const WINDOW_BOUNDS: Shape = Record(&[
    ("x", Scalar),
    ("y", Scalar),
    ("width", Scalar),
    ("height", Scalar),
    ("maximized", Scalar),
]);

const SNAPSHOT_FIELDS: &[(&str, Shape)] = &[
    ("spaces", Array(&SPACE)),
    ("activeSpaceId", Scalar),
    ("windows", Array(&WINDOW)),
    ("closedWindows", Array(&WINDOW)),
    ("favorites", TAB_IDS),
    ("archive", Array(&ARCHIVE_ENTRY)),
    ("sidebarVisible", Scalar),
    ("sidebarWidth", Scalar),
    ("theme", Scalar),
    ("searchEngine", Scalar),
    ("archiveAfterHours", Scalar),
    ("keybindings", Dictionary(&Scalar)),
    ("environments", Array(&ENVIRONMENT)),
    ("window", WINDOW_BOUNDS),
    ("peekPinnedLinks", Scalar),
    ("openLinksInFlechinha", Scalar),
    ("sidebarInPageFullscreen", Scalar),
    ("downloadsFolder", Scalar),
    ("askWhereToSave", Scalar),
    ("adBlock", Scalar),
    ("adBlockAllowed", Array(&Scalar)),
    ("onboarded", Scalar),
    ("sitePermissions", Dictionary(&Dictionary(&Scalar))),
];

pub(super) fn project(
    fields: &RawRecord<'_>,
    repairs: &mut SnapshotRepairs,
) -> Result<Map<String, Value>, DecodeError> {
    // IgnoredAny scans unknown subtrees without materializing them.
    SNAPSHOT_FIELDS
        .iter()
        .filter_map(|&(key, shape)| Some((key, shape, fields.get(key)?)))
        .map(|(key, shape, raw)| {
            let field = Projection { shape, repairs };
            Ok((key.into(), field.project(raw).map_err(DecodeError::Json)?))
        })
        .collect()
}

struct Projection<'a> {
    shape: Shape,
    repairs: &'a mut SnapshotRepairs,
}

impl Projection<'_> {
    fn project(self, raw: &RawValue) -> serde_json::Result<Value> {
        match (self.shape, raw.get().as_bytes().first()) {
            (Scalar, _) => Ok(scalar(raw)),
            (LegacyPins, _) => {
                legacy_pins::project(raw, &mut self.repairs.flattened_legacy_folders)
            }
            (Array(_), Some(b'[')) | (Record(_) | Dictionary(_), Some(b'{')) => {
                let mut deserializer = serde_json::Deserializer::from_str(raw.get());
                serde::Deserializer::deserialize_any(&mut deserializer, self)
            }
            // Leave a wrong-type value for repair rather than asking the JSON
            // deserializer to interpret it as a number, string or container.
            _ => Ok(Value::Null),
        }
    }
}

impl<'de> DeserializeSeed<'de> for Projection<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.project(<&RawValue>::deserialize(deserializer)?)
            .map_err(D::Error::custom)
    }
}

pub(super) fn scalar(raw: &RawValue) -> Value {
    scalar_text(raw.get())
}

pub(super) fn scalar_text(raw: &str) -> Value {
    // Preserve the wrong container type for the repair checks. In particular, a
    // malformed shortcut must not turn into null, which means an explicit unbind.
    match raw.as_bytes().first() {
        Some(b'{') => Value::Object(Map::new()),
        Some(b'[') => Value::Array(Vec::new()),
        Some(b'"') => serde_json::from_str::<String>(raw)
            .or_else(|_| lossy_string(raw))
            .map_or_else(|_| Value::Object(Map::new()), Value::String),
        _ => serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(Map::new())),
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct SavedKey(String);

impl<'de> Deserialize<'de> for SavedKey {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        input.deserialize_bytes(SavedString).map(Self)
    }
}

struct SavedString;

impl Visitor<'_> for SavedString {
    type Value = String;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON string")
    }

    fn visit_bytes<E>(self, bytes: &[u8]) -> Result<String, E> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Ok(text.into());
        }
        let mut bytes = bytes.to_vec();
        // serde_json's byte reader decodes valid pairs and represents each lone
        // UTF-16 surrogate as one WTF-8 triplet. Replace each triplet once.
        for start in 0..bytes.len().saturating_sub(2) {
            let triplet = &mut bytes[start..start + 3];
            if matches!(triplet, [0xed, 0xa0..=0xbf, 0x80..=0xbf]) {
                triplet.copy_from_slice("\u{fffd}".as_bytes());
            }
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

pub(super) fn lossy_string(json: &str) -> serde_json::Result<String> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    serde::Deserializer::deserialize_bytes(&mut deserializer, SavedString)
}

pub(super) fn read_record(json: &str) -> serde_json::Result<RawRecord<'_>> {
    let fields: std::collections::BTreeMap<SavedKey, &RawValue> = serde_json::from_str(json)?;
    Ok(fields
        .into_iter()
        .map(|(SavedKey(key), value)| (key, value))
        .collect())
}

impl<'de> Visitor<'de> for Projection<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a snapshot value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(SavedKey(key)) = input.next_key()? {
            let shape = match self.shape {
                Dictionary(shape) => Some(*shape),
                Record(fields) => fields
                    .iter()
                    .find_map(|&(name, shape)| (key == name).then_some(shape)),
                _ => None,
            };
            if let Some(shape) = shape {
                let value = input.next_value_seed(Projection {
                    shape,
                    repairs: self.repairs,
                })?;
                result.insert(key, value);
            } else {
                input.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Value::Object(result))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut result = Vec::new();
        if let Array(shape) = self.shape {
            while let Some(value) = input.next_element_seed(Projection {
                shape: *shape,
                repairs: self.repairs,
            })? {
                result.push(value);
            }
        } else {
            while input.next_element::<IgnoredAny>()?.is_some() {}
        }
        Ok(Value::Array(result))
    }
}
