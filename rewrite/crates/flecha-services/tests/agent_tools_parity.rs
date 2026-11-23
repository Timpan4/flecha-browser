//! The Rust server advertises and accepts what the TS agent bridge does. The fixture is recorded
//! from `chromium/shared/agentTools.ts` through the TS SDK by `scripts/agent-tools-fixture.mjs`;
//! CI regenerates it and fails when it is stale.
//!
//! Every difference must be listed in `DIFFERENCES` with its reason, and every listed difference
//! must still occur, so the list cannot go stale either.

use std::cell::RefCell;
use std::collections::BTreeSet;

use flecha_services::agent::tools::{self, MAX_TOOL_WAIT, TOOL_TIMEOUT};
use flecha_services::agent::{PROTOCOL_CEILING, server_config, tool_list};
use rmcp::model::ProtocolVersion;
use serde_json::{Map, Value};

const FIXTURE: &str = include_str!("fixtures/agent-tools.json");

/// Where Rust and TS knowingly differ: (place, reason).
const DIFFERENCES: &[(&str, &str)] = &[
    (
        "tools/*/execution",
        "rmcp 3.11.0's Tool has no `execution` member. The TS SDK sends \
         `{\"taskSupport\":\"forbidden\"}`, and the 2025-11-25 spec reads an absent \
         `taskSupport` as \"forbidden\", so clients see the same thing.",
    ),
    (
        "protocolVersions/2024-10-07",
        "rmcp 3.11.0 does not know the 2024-10-07 pre-release revision. Clients that ask for it \
         are offered 2025-11-25, as the TS SDK does for any unsupported version.",
    ),
];

thread_local! {
    static USED: RefCell<BTreeSet<&'static str>> = const { RefCell::new(BTreeSet::new()) };
}

/// Whether `place` differs knowingly; records the use.
fn allowed(place: &str) -> bool {
    match DIFFERENCES.iter().find(|(known, _)| *known == place) {
        Some((known, _)) => {
            USED.with(|used| used.borrow_mut().insert(known));
            true
        }
        None => false,
    }
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

/// Numbers compared by value, so 10 and 10.0 agree as they do in JavaScript.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(number) => serde_json::json!(number.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalized(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
fn tools_match_agent_tools_ts() {
    let fixture = fixture();
    let expected = fixture["tools"].as_array().unwrap();
    let actual: Vec<Value> = tool_list()
        .iter()
        .map(|tool| serde_json::to_value(tool).unwrap())
        .collect();
    let names = |tools: &[Value]| {
        tools
            .iter()
            .map(|tool| tool["name"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&actual), names(expected), "tool names or order");
    let mut mismatches = Vec::new();
    for (actual, expected) in actual.iter().zip(expected) {
        let name = expected["name"].as_str().unwrap();
        let keys: BTreeSet<&String> = actual
            .as_object()
            .unwrap()
            .keys()
            .chain(expected.as_object().unwrap().keys())
            .collect();
        for key in keys {
            if actual.get(key.as_str()) == expected.get(key.as_str()) {
                continue;
            }
            if !allowed(&format!("tools/*/{key}")) {
                mismatches.push(format!(
                    "{name}.{key}: rust {} ts {}",
                    actual.get(key.as_str()).unwrap_or(&Value::Null),
                    expected.get(key.as_str()).unwrap_or(&Value::Null)
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "unrecorded differences:\n{}",
        mismatches.join("\n")
    );
    assert_used(&["tools/*/execution"]);
}

#[test]
fn server_identity_and_capabilities_match() {
    let fixture = fixture();
    let config = serde_json::to_value(server_config()).unwrap();
    assert_eq!(config["serverInfo"], fixture["serverInfo"]);
    assert_eq!(config["capabilities"], fixture["capabilities"]);
}

#[test]
fn protocol_versions_match() {
    let fixture = fixture();
    let ts: BTreeSet<&str> = fixture["protocolVersions"]["supported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version.as_str().unwrap())
        .collect();
    let rust: BTreeSet<&str> = ProtocolVersion::known_up_to(&PROTOCOL_CEILING)
        .iter()
        .map(ProtocolVersion::as_str)
        .collect();
    assert_eq!(
        PROTOCOL_CEILING.as_str(),
        fixture["protocolVersions"]["latest"]
    );
    for version in rust.symmetric_difference(&ts) {
        assert!(
            allowed(&format!("protocolVersions/{version}")),
            "unrecorded protocol difference {version}"
        );
    }
    assert_used(&["protocolVersions/2024-10-07"]);
}

#[test]
fn limits_match_native_messaging_ts() {
    let fixture = fixture();
    assert_eq!(
        fixture["constants"]["TOOL_TIMEOUT"],
        TOOL_TIMEOUT.as_millis() as u64
    );
    assert_eq!(fixture["constants"]["MAX_TOOL_WAIT"], MAX_TOOL_WAIT);
}

#[test]
fn arguments_are_judged_as_zod_judges_them() {
    let fixture = fixture();
    let mut mismatches = Vec::new();
    for case in fixture["validation"].as_array().unwrap() {
        let tool = case["tool"].as_str().unwrap();
        let raw = case["arguments"].as_str();
        let arguments: Option<Map<String, Value>> =
            raw.map(|raw| serde_json::from_str(raw).unwrap());
        let verdict = tools::find(tool).unwrap().validate(arguments.as_ref());
        let label = format!("{tool} {}", raw.unwrap_or("(no arguments)"));
        match (verdict, case["ok"].as_bool().unwrap()) {
            (Ok(args), true) => {
                let parsed = Value::Object(args.as_map().clone());
                if normalized(&parsed) != normalized(&case["parsed"]) {
                    mismatches.push(format!("{label}: parsed {parsed} vs {}", case["parsed"]));
                }
            }
            (Err(message), false) => {
                if message != case["error"].as_str().unwrap() {
                    mismatches.push(format!(
                        "{label}: message {message:?} vs {:?}",
                        case["error"]
                    ));
                }
            }
            (Ok(args), false) => mismatches.push(format!(
                "{label}: accepted {:?}, zod refused: {}",
                args.as_map(),
                case["error"]
            )),
            (Err(message), true) => {
                mismatches.push(format!("{label}: refused ({message}), zod accepted"))
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

fn assert_used(places: &[&str]) {
    for place in places {
        assert!(
            USED.with(|used| used.borrow().contains(place)),
            "recorded difference {place} no longer occurs: remove it from DIFFERENCES"
        );
    }
}
