//! The agent tools: names, descriptions and input schemas as `chromium/shared/agentTools.ts`
//! declares them, and argument validation that gives zod's verdicts and messages.
//!
//! `tests/agent_tools_parity.rs` compares both against a fixture recorded from the TS SDK.

use std::time::Duration;

use serde_json::{Map, Value, json};

/// How long a tool may take before Flecha gives up on it (`TOOL_TIMEOUT` in
/// `chromium/shared/nativeMessaging.ts`).
pub const TOOL_TIMEOUT: Duration = Duration::from_millis(60_000);

/// The longest `wait_for` timeout (`MAX_TOOL_WAIT` in `chromium/shared/nativeMessaging.ts`):
/// Node's signed 32-bit timer limit less two tool allowances.
pub const MAX_TOOL_WAIT: u64 = 2_147_483_647 - 2 * 60_000;

/// `wait_for` without `timeoutMs` (the default in `chromium/shell/nativeHost.ts`).
pub const DEFAULT_WAIT_MS: u64 = 10_000;

/// How often `wait_for` probes the page (`wait(200)` in `chromium/shell/nativeHost.ts`).
pub const WAIT_POLL: Duration = Duration::from_millis(200);

/// `read_page` without `maxChars` (the default in `chromium/shell/nativeHost.ts`).
pub const DEFAULT_MAX_CHARS: f64 = 40_000.0;

/// `console_logs` without `limit` (the default in `chromium/shell/nativeHost.ts`).
pub const DEFAULT_CONSOLE_LIMIT: f64 = 50.0;

/// JavaScript's `Number.MAX_SAFE_INTEGER`, which zod's `.int()` enforces.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

const DRAFT_07: &str = "http://json-schema.org/draft-07/schema#";
const TAB_ID_DESCRIPTION: &str = "Flecha tab id, from open_tab or list_tabs";

#[derive(Clone, Copy, Debug)]
enum Kind {
    String,
    Boolean,
    Number,
    /// `z.number().int().min(min).max(max)`.
    Integer {
        min: f64,
        max: f64,
    },
    Enum(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
struct Field {
    name: &'static str,
    kind: Kind,
    required: bool,
    description: Option<&'static str>,
}

const fn required(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        required: true,
        description: None,
    }
}

const fn optional(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        required: false,
        description: None,
    }
}

/// The shared `tabId` field; `open_tab`'s siblings declare a plain string instead.
const TAB_ID: Field = Field {
    name: "tabId",
    kind: Kind::String,
    required: true,
    description: Some(TAB_ID_DESCRIPTION),
};

/// One tool as `AGENT_TOOLS` declares it. `fields: None` is a tool without `inputSchema`.
#[derive(Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    fields: Option<&'static [Field]>,
}

/// Every agent tool, in `AGENT_TOOLS` order.
pub static TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "list_tabs",
        description: "List open tabs in Flecha with their ids, titles, URLs and Space. `active` marks the tabs the user is looking at; `agent` marks tabs coding agents opened.",
        fields: None,
    },
    ToolSpec {
        name: "open_tab",
        description: "Open a URL in a new background tab, in the sidebar's agent folder, and return its id for the other tools. The user's view and keyboard stay where they are; the other tools work on the tab without showing it.",
        fields: Some(&[required("url", Kind::String)]),
    },
    ToolSpec {
        name: "activate_tab",
        description: "Switch the user's view to a tab. Only when the user asks to see it: the other tools don't need the tab on screen.",
        fields: Some(&[required("tabId", Kind::String)]),
    },
    ToolSpec {
        name: "close_tab",
        description: "Close a tab (pinned tabs and favorites are unloaded instead).",
        fields: Some(&[required("tabId", Kind::String)]),
    },
    ToolSpec {
        name: "navigate",
        description: "Load a URL in a tab, or go back/forward/reload, and wait for the page to settle.",
        fields: Some(&[
            TAB_ID,
            optional("url", Kind::String),
            optional("action", Kind::Enum(&["back", "forward", "reload"])),
        ]),
    },
    ToolSpec {
        name: "read_page",
        description: "Read a tab's content: visible text (default), full HTML, or its links.",
        fields: Some(&[
            TAB_ID,
            optional("format", Kind::Enum(&["text", "html", "links"])),
            optional("maxChars", Kind::Number),
        ]),
    },
    ToolSpec {
        name: "screenshot",
        description: "Screenshot a tab's viewport, in the background.",
        fields: Some(&[TAB_ID]),
    },
    ToolSpec {
        name: "console_logs",
        description: "A tab's recent console messages (errors, warnings, logs), newest last.",
        fields: Some(&[
            TAB_ID,
            optional("level", Kind::Enum(&["error", "warning", "info", "debug"])),
            optional("limit", Kind::Number),
        ]),
    },
    ToolSpec {
        name: "click",
        description: "Click the element matching a CSS selector, with a real mouse event.",
        fields: Some(&[TAB_ID, required("selector", Kind::String)]),
    },
    ToolSpec {
        name: "type",
        description: "Type text into the element matching a selector (or the focused one), optionally clearing it first and pressing Enter.",
        fields: Some(&[
            TAB_ID,
            optional("selector", Kind::String),
            required("text", Kind::String),
            optional("clear", Kind::Boolean),
            optional("submit", Kind::Boolean),
        ]),
    },
    ToolSpec {
        name: "press_key",
        description: "Press a key such as Enter, Tab, Escape or ArrowDown in a tab.",
        fields: Some(&[TAB_ID, required("key", Kind::String)]),
    },
    ToolSpec {
        name: "evaluate",
        description: "Run JavaScript in a tab's page and return the result (promises are awaited, values are JSON-serialized).",
        fields: Some(&[TAB_ID, required("expression", Kind::String)]),
    },
    ToolSpec {
        name: "wait_for",
        description: "Wait until an element matching a selector exists (or the page finishes loading when no selector is given).",
        fields: Some(&[
            TAB_ID,
            optional("selector", Kind::String),
            optional(
                "timeoutMs",
                Kind::Integer {
                    min: 0.0,
                    max: MAX_TOOL_WAIT as f64,
                },
            ),
        ]),
    },
];

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

impl ToolSpec {
    /// The JSON Schema the TS SDK derives from the zod shape.
    pub fn input_schema(&self) -> Map<String, Value> {
        let mut schema = Map::new();
        schema.insert("type".into(), json!("object"));
        let Some(fields) = self.fields else {
            schema.insert("properties".into(), json!({}));
            return schema;
        };
        let properties = fields
            .iter()
            .map(|field| (field.name.to_owned(), field.schema()))
            .collect::<Map<_, _>>();
        schema.insert("properties".into(), Value::Object(properties));
        let required: Vec<_> = fields
            .iter()
            .filter(|field| field.required)
            .map(|field| field.name)
            .collect();
        if !required.is_empty() {
            schema.insert("required".into(), json!(required));
        }
        schema.insert("$schema".into(), json!(DRAFT_07));
        schema
    }

    /// Checks arguments as the SDK's zod parse does: the first issue per field in shape order,
    /// unknown members dropped. A tool without `inputSchema` runs with no arguments, as
    /// `chromium/native-host/host.ts` calls it.
    pub fn validate(&self, arguments: Option<&Map<String, Value>>) -> Result<Args, String> {
        let empty = Map::new();
        let arguments = arguments.unwrap_or(&empty);
        let Some(fields) = self.fields else {
            return Ok(Args(Map::new()));
        };
        let mut parsed = Map::new();
        let mut issues = Vec::new();
        for field in fields {
            match arguments.get(field.name) {
                None if !field.required => {}
                value => match field.check(value) {
                    Ok(value) => {
                        parsed.insert(field.name.to_owned(), value);
                    }
                    Err(found) => issues.extend(
                        found
                            .into_iter()
                            .map(|issue| format!("{issue} at {}", field.name)),
                    ),
                },
            }
        }
        if issues.is_empty() {
            Ok(Args(parsed))
        } else {
            Err(format!(
                "MCP error -32602: Input validation error: Invalid arguments for tool {}: {}",
                self.name,
                issues.join("\n")
            ))
        }
    }
}

impl Field {
    fn schema(&self) -> Value {
        let mut schema = Map::new();
        match self.kind {
            Kind::String => {
                schema.insert("type".into(), json!("string"));
            }
            Kind::Boolean => {
                schema.insert("type".into(), json!("boolean"));
            }
            Kind::Number => {
                schema.insert("type".into(), json!("number"));
            }
            Kind::Integer { min, max } => {
                schema.insert("type".into(), json!("integer"));
                schema.insert("minimum".into(), json!(min as u64));
                schema.insert("maximum".into(), json!(max as u64));
            }
            Kind::Enum(options) => {
                schema.insert("type".into(), json!("string"));
                schema.insert("enum".into(), json!(options));
            }
        }
        if let Some(description) = self.description {
            schema.insert("description".into(), json!(description));
        }
        Value::Object(schema)
    }

    fn check(&self, value: Option<&Value>) -> Result<Value, Vec<String>> {
        let mismatch = |expected: &str| {
            vec![format!(
                "Invalid input: expected {expected}, received {}",
                received(value)
            )]
        };
        match (self.kind, value) {
            (Kind::String, Some(Value::String(_))) | (Kind::Boolean, Some(Value::Bool(_))) => {
                Ok(value.cloned().unwrap_or_default())
            }
            (Kind::String, _) => Err(mismatch("string")),
            (Kind::Boolean, _) => Err(mismatch("boolean")),
            (Kind::Number, Some(Value::Number(number))) => Ok(json!(as_f64(number))),
            (Kind::Number, _) => Err(mismatch("number")),
            (Kind::Enum(options), Some(Value::String(choice)))
                if options.contains(&choice.as_str()) =>
            {
                Ok(json!(choice))
            }
            (Kind::Enum(options), _) => Err(vec![format!(
                "Invalid option: expected one of {}",
                options
                    .iter()
                    .map(|option| format!("\"{option}\""))
                    .collect::<Vec<_>>()
                    .join("|")
            )]),
            (Kind::Integer { min, max }, Some(Value::Number(number))) => {
                let number = as_f64(number);
                if number.fract() != 0.0 {
                    return Err(vec![
                        "Invalid input: expected int, received number".to_owned(),
                    ]);
                }
                let mut issues = Vec::new();
                if number > MAX_SAFE_INTEGER {
                    issues.push(format!(
                        "Too big: expected int to be <={}",
                        MAX_SAFE_INTEGER as u64
                    ));
                } else if number < -MAX_SAFE_INTEGER {
                    issues.push(format!(
                        "Too small: expected int to be >=-{}",
                        MAX_SAFE_INTEGER as u64
                    ));
                }
                if number < min {
                    issues.push(format!("Too small: expected number to be >={min}"));
                } else if number > max {
                    issues.push(format!("Too big: expected number to be <={max}"));
                }
                if issues.is_empty() {
                    // -0 reads as 0, as JSON.stringify writes it.
                    Ok(json!(number.abs() as u64))
                } else {
                    Err(issues)
                }
            }
            (Kind::Integer { .. }, _) => Err(mismatch("number")),
        }
    }
}

/// zod's name for what it found.
fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// A JSON number as JavaScript's `JSON.parse` reads it.
fn as_f64(number: &serde_json::Number) -> f64 {
    number.as_f64().unwrap_or(f64::NAN)
}

/// Arguments that passed [`ToolSpec::validate`], with unknown members dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(Map<String, Value>);

impl Args {
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    pub fn str(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(Value::as_str)
    }

    pub fn bool(&self, name: &str) -> bool {
        self.0.get(name).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        self.0.get(name).and_then(Value::as_f64)
    }

    pub fn integer(&self, name: &str) -> Option<u64> {
        self.0.get(name).and_then(Value::as_u64)
    }
}
