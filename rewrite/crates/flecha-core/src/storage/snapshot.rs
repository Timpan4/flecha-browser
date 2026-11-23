use serde::Deserialize;
use serde::de::{DeserializeSeed, Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value, value::RawValue};

use super::{DecodeError, RawRecord};

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

pub(super) fn project(fields: &RawRecord<'_>) -> Result<Map<String, Value>, DecodeError> {
    // IgnoredAny scans unknown subtrees without materializing them.
    SNAPSHOT_FIELDS
        .iter()
        .filter_map(|&(key, shape)| Some((key, shape, fields.get(key)?)))
        .map(|(key, shape, raw)| {
            let mut deserializer = serde_json::Deserializer::from_str(raw.get());
            let value = shape
                .deserialize(&mut deserializer)
                .map_err(DecodeError::Json)?;
            Ok((key.into(), value))
        })
        .collect()
}

impl<'de> DeserializeSeed<'de> for Shape {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        match self {
            Scalar => scalar(<&RawValue>::deserialize(deserializer)?).map_err(D::Error::custom),
            // The enclosing spaces array and Space record count toward JSON depth.
            LegacyPins => legacy_pins::<D::Error>(<&RawValue>::deserialize(deserializer)?, 2),
            _ => deserializer.deserialize_any(self),
        }
    }
}

fn scalar(raw: &RawValue) -> serde_json::Result<Value> {
    // Preserve the wrong container type for the repair checks. In particular, a
    // malformed shortcut must not turn into null, which means an explicit unbind.
    match raw.get().as_bytes().first() {
        Some(b'{') => Ok(Value::Object(Map::new())),
        Some(b'[') => Ok(Value::Array(Vec::new())),
        _ => serde_json::from_str(raw.get()),
    }
}

fn legacy_pins<E: Error>(raw: &RawValue, parent_depth: usize) -> Result<Value, E> {
    // Preserve serde_json's default recursion limit for the legacy nested format.
    // RawValue defers the check so only real folder children are counted.
    const JSON_RECURSION_LIMIT: usize = 128;
    if !raw.get().starts_with('[') {
        return Ok(Value::Null);
    }
    let array_depth = parent_depth + 1;
    if array_depth >= JSON_RECURSION_LIMIT {
        return Err(E::custom(
            "legacy pin tree exceeds the JSON recursion limit",
        ));
    }
    let nodes: Vec<&RawValue> = serde_json::from_str(raw.get()).map_err(E::custom)?;
    let mut projected = Vec::new();
    for node in nodes {
        let Ok(record) = serde_json::from_str::<RawRecord<'_>>(node.get()) else {
            continue;
        };
        let depth = array_depth + 1;
        if depth >= JSON_RECURSION_LIMIT {
            return Err(E::custom(
                "legacy pin tree exceeds the JSON recursion limit",
            ));
        }
        let mut fields = Map::new();
        for key in ["id", "kind", "tabId", "name", "open"] {
            if let Some(value) = record.get(key) {
                fields.insert(key.into(), scalar(value).map_err(E::custom)?);
            }
        }
        if fields.get("kind").and_then(Value::as_str) == Some("folder")
            && fields.get("id").is_some_and(Value::is_string)
            && let Some(children) = record.get("children")
        {
            fields.insert("children".into(), legacy_pins::<E>(children, depth)?);
        }
        projected.push(Value::Object(fields));
    }
    Ok(Value::Array(projected))
}

impl<'de> Visitor<'de> for Shape {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a snapshot value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(key) = input.next_key::<String>()? {
            let shape = match self {
                Dictionary(shape) => Some(*shape),
                Record(fields) => fields
                    .iter()
                    .find_map(|&(name, shape)| (key == name).then_some(shape)),
                _ => None,
            };
            if let Some(shape) = shape {
                result.insert(key, input.next_value_seed(shape)?);
            } else {
                input.next_value::<IgnoredAny>()?;
            }
        }
        Ok(Value::Object(result))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let mut result = Vec::new();
        if let Array(shape) = self {
            while let Some(value) = input.next_element_seed(*shape)? {
                result.push(value);
            }
        } else {
            while input.next_element::<IgnoredAny>()?.is_some() {}
        }
        Ok(Value::Array(result))
    }

    fn visit_bool<E>(self, _: bool) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_str<E>(self, _: &str) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}
