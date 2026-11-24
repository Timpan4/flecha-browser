//! What each tool does against the fake backend: agent tab placement through flecha-core, which
//! tabs a tool may reach, result formats, and deadlines.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use flecha_core::{BrowserState, CloseToday, Placement, SpaceId, Tab, TabId, WindowId};
use flecha_services::agent::fake::{FakeBrowser, FakePage};
use flecha_services::agent::{ConsoleEntry, ConsoleLevel, Evaluated, call};
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

const TAB_TOOLS: [&str; 11] = [
    "activate_tab",
    "close_tab",
    "navigate",
    "read_page",
    "screenshot",
    "console_logs",
    "click",
    "type",
    "press_key",
    "evaluate",
    "wait_for",
];

fn args(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

/// Valid arguments for a tab tool, beyond `tabId`.
fn extra(tool: &str) -> Value {
    match tool {
        "click" => json!({ "selector": "#go" }),
        "type" => json!({ "text": "hi" }),
        "press_key" => json!({ "key": "Enter" }),
        "evaluate" => json!({ "expression": "1" }),
        "wait_for" => json!({ "timeoutMs": 1000 }),
        _ => json!({}),
    }
}

async fn run(browser: &FakeBrowser, tool: &str, arguments: Value) -> CallToolResult {
    call(browser, tool, Some(&args(arguments))).await
}

fn text(result: &CallToolResult) -> &str {
    &result.content[0].as_text().unwrap().text
}

fn failed(result: &CallToolResult) -> bool {
    result.is_error == Some(true)
}

/// Two windows: the user's tab in the focused one, a pinned tab, a favorite.
fn browser() -> (FakeBrowser, TabId, TabId, TabId) {
    let space = SpaceId::new("space-1");
    let window = WindowId::new("window-1");
    let mut state = BrowserState::new(space.clone(), window.clone());
    let mine = TabId::new("mine");
    assert!(state.open_today(
        mine.clone(),
        Tab::new("https://example.com/mine", 1),
        &window,
        None,
        true
    ));
    let pinned = TabId::new("pinned");
    assert!(state.create_tab(
        pinned.clone(),
        Tab::new("https://example.com/pinned", 1),
        Placement::Pinned {
            space: &space,
            folder: None
        },
        None
    ));
    let favorite = TabId::new("favorite");
    assert!(state.create_tab(
        favorite.clone(),
        Tab::new("https://example.com/favorite", 1),
        Placement::Favorite,
        None
    ));
    (FakeBrowser::new(state), mine, pinned, favorite)
}

/// Everything that decides what the user sees and where their keys go.
fn view(browser: &FakeBrowser) -> String {
    let state = browser.state();
    format!(
        "{:?} {:?}",
        state.focused_window().id,
        state
            .windows()
            .iter()
            .map(|w| (&w.id, &w.active_space_id, w.visible_tabs().to_vec()))
            .collect::<Vec<_>>()
    )
}

#[tokio::test]
async fn open_tab_places_an_agent_tab_without_showing_it() {
    let (browser, mine, ..) = browser();
    let before = view(&browser);
    let opened = run(&browser, "open_tab", json!({ "url": "example.org/page" })).await;
    assert!(!failed(&opened), "{}", text(&opened));
    let reply: Value = serde_json::from_str(text(&opened)).unwrap();
    assert_eq!(reply["url"], "https://example.org/page");
    let id = TabId::new(reply["tabId"].as_str().unwrap());
    assert_eq!(view(&browser), before, "open_tab changed the user's view");
    let state = browser.state();
    let window = state.focused_window();
    let today = &window.spaces[&window.active_space_id].today;
    assert_eq!(
        today.first(),
        Some(&id),
        "the agent tab goes first in the focused window's Today"
    );
    let tab = state.tab(&id).unwrap();
    assert!(tab.agent && tab.unseen);
    assert!(state.in_agent_folder(&window.id, &id));
    assert!(!state.in_agent_folder(&window.id, &mine));
}

#[tokio::test]
async fn open_tab_resolves_text_as_the_command_bar_does() {
    // Expected values are resolveInput(input, "google") from chromium/shell/lib/url.ts.
    let (browser, ..) = browser();
    for (input, url, title) in [
        (
            "rust traits",
            "https://www.google.com/search?q=rust%20traits",
            None,
        ),
        ("", "about:blank", Some("New Tab")),
        ("   ", "about:blank", Some("New Tab")),
        ("user:pass@host.example/p", "about:blank", Some("New Tab")),
        (
            "https://user:pw@example.com/",
            "https://user:pw@example.com/",
            None,
        ),
    ] {
        let reply: Value = serde_json::from_str(text(
            &run(&browser, "open_tab", json!({ "url": input })).await,
        ))
        .unwrap();
        assert_eq!(reply["url"], url, "{input:?}");
        let state = browser.state();
        let tab = state
            .tab(&TabId::new(reply["tabId"].as_str().unwrap()))
            .unwrap();
        assert_eq!(tab.title, title.unwrap_or(url), "{input:?}");
    }
}

#[tokio::test]
async fn clearing_today_leaves_the_agent_folder() {
    let (browser, mine, ..) = browser();
    let reply: Value = serde_json::from_str(text(
        &run(
            &browser,
            "open_tab",
            json!({ "url": "https://example.org/" }),
        )
        .await,
    ))
    .unwrap();
    let agent = TabId::new(reply["tabId"].as_str().unwrap());
    let mut state = browser.state();
    let window = state.focused_window().id.clone();
    let closed = state.clear_today(&window, 10);
    assert!(closed.contains(&mine));
    assert!(!closed.contains(&agent));
    assert!(state.tab(&agent).is_some());
    assert!(
        state
            .close_today_tabs(&agent, CloseToday::Others, 11)
            .is_empty()
    );
}

#[tokio::test]
async fn list_tabs_reports_every_zone_and_the_agent_flag() {
    let (browser, ..) = browser();
    run(
        &browser,
        "open_tab",
        json!({ "url": "https://example.org/" }),
    )
    .await;
    let listed: Value = serde_json::from_str(text(
        &run(&browser, "list_tabs", json!({ "ignored": true })).await,
    ))
    .unwrap();
    let rows: Vec<_> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap().to_owned(),
                row["zone"].as_str().unwrap().to_owned(),
                row["active"].as_bool().unwrap(),
                row["agent"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("favorite".into(), "favorites".into(), false, false),
            ("pinned".into(), "pinned".into(), false, false),
            ("agent-1".into(), "today".into(), false, true),
            ("mine".into(), "today".into(), true, false),
        ]
    );
    // Field names and order as `AgentTab` in agentTools.ts.
    let keys: Vec<_> = listed[0].as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["id", "title", "url", "space", "zone", "active", "agent"]
    );
    assert_eq!(listed[0]["space"], "");
}

#[tokio::test]
async fn only_activate_tab_changes_the_users_view() {
    let (browser, mine, pinned, favorite) = browser();
    let agent_reply: Value = serde_json::from_str(text(
        &run(
            &browser,
            "open_tab",
            json!({ "url": "https://example.org/" }),
        )
        .await,
    ))
    .unwrap();
    let agent = agent_reply["tabId"].as_str().unwrap().to_owned();
    for tab in [&agent, mine.as_str(), pinned.as_str(), favorite.as_str()] {
        browser.page(&TabId::new(tab), |page| {
            page.elements.insert("#go".into());
        });
    }
    let before = view(&browser);
    for tab in [&agent, mine.as_str(), pinned.as_str(), favorite.as_str()] {
        for tool in TAB_TOOLS
            .iter()
            .filter(|tool| !matches!(**tool, "activate_tab" | "close_tab"))
        {
            let mut arguments = extra(tool);
            arguments["tabId"] = json!(tab);
            let result = run(&browser, tool, arguments).await;
            assert!(!failed(&result), "{tool} on {tab}: {}", text(&result));
            assert_eq!(view(&browser), before, "{tool} on {tab} changed the view");
        }
    }
    run(&browser, "list_tabs", json!({})).await;
    assert_eq!(view(&browser), before);
    assert!(!failed(
        &run(&browser, "activate_tab", json!({ "tabId": agent })).await
    ));
    assert_ne!(view(&browser), before);
    let state = browser.state();
    assert_eq!(
        state.focused_window().visible_tabs(),
        [TabId::new(agent.as_str())]
    );
}

#[tokio::test]
async fn every_tab_tool_refuses_an_unknown_tab() {
    let (browser, ..) = browser();
    for tool in TAB_TOOLS {
        let mut arguments = extra(tool);
        arguments["tabId"] = json!("nope");
        let result = run(&browser, tool, arguments).await;
        assert!(failed(&result), "{tool}");
        assert_eq!(text(&result), "No tab with id nope", "{tool}");
    }
}

#[tokio::test]
async fn tabs_outside_the_regular_state_are_out_of_reach() {
    // An engine page the state does not list, as an incognito window's tab.
    let (browser, ..) = browser();
    let hidden = TabId::new("incognito-1");
    browser.add_detached_page(
        &hidden,
        FakePage {
            url: "https://private.example/".into(),
            text: "private".into(),
            elements: ["#go".into()].into(),
            loaded: true,
            ..FakePage::default()
        },
    );
    for tool in TAB_TOOLS {
        let mut arguments = extra(tool);
        arguments["tabId"] = json!(hidden.as_str());
        let result = run(&browser, tool, arguments).await;
        assert_eq!(text(&result), "No tab with id incognito-1", "{tool}");
    }
    let listed = text(&run(&browser, "list_tabs", json!({})).await).to_owned();
    assert!(!listed.contains("incognito-1"));
}

#[tokio::test]
async fn evaluate_and_read_page_reach_user_tabs_as_in_the_shell() {
    // The shell lets agents read and script every regular tab, the user's included.
    let (browser, mine, pinned, favorite) = browser();
    for tab in [&mine, &pinned, &favorite] {
        browser.page(tab, |page| {
            page.text = format!("text of {}", tab.as_str());
            page.evaluations.insert(
                "document.title".into(),
                Evaluated::Value(json!(tab.as_str())),
            );
        });
        let read = run(&browser, "read_page", json!({ "tabId": tab.as_str() })).await;
        assert!(text(&read).ends_with(&format!("\n\ntext of {}", tab.as_str())));
        let evaluated = run(
            &browser,
            "evaluate",
            json!({ "tabId": tab.as_str(), "expression": "document.title" }),
        )
        .await;
        assert_eq!(text(&evaluated), tab.as_str());
    }
}

#[tokio::test]
async fn a_tab_closed_while_a_tool_waits_fails_the_tool() {
    let (browser, ..) = browser();
    let browser = Arc::new(browser);
    let reply: Value = serde_json::from_str(text(
        &run(
            &browser,
            "open_tab",
            json!({ "url": "https://example.org/" }),
        )
        .await,
    ))
    .unwrap();
    let tab = reply["tabId"].as_str().unwrap().to_owned();
    let waiting = tokio::spawn({
        let browser = browser.clone();
        let tab = tab.clone();
        async move {
            run(
                &browser,
                "wait_for",
                json!({ "tabId": tab, "selector": "#never", "timeoutMs": 60_000 }),
            )
            .await
        }
    });
    while browser.stats.probes.load(Ordering::Relaxed) == 0 {
        tokio::task::yield_now().await;
    }
    assert!(!failed(
        &run(&browser, "close_tab", json!({ "tabId": tab })).await
    ));
    let result = waiting.await.unwrap();
    assert_eq!(
        text(&result),
        format!("Tab {tab} closed before the tool could run")
    );
}

#[tokio::test]
async fn close_tab_unloads_pinned_tabs_and_favorites() {
    let (browser, mine, pinned, favorite) = browser();
    for tab in [&pinned, &favorite] {
        assert_eq!(
            text(&run(&browser, "close_tab", json!({ "tabId": tab.as_str() })).await),
            "ok"
        );
        assert!(browser.state().tab(tab).is_some(), "{tab:?} kept");
    }
    assert_eq!(
        text(&run(&browser, "close_tab", json!({ "tabId": mine.as_str() })).await),
        "ok"
    );
    assert!(browser.state().locate(&mine).is_none());
}

#[tokio::test]
async fn read_page_formats_and_cuts_as_the_shell_does() {
    let (browser, mine, ..) = browser();
    browser.page(&mine, |page| {
        page.title = "Mine".into();
        page.text = "abcdef".into();
        page.html = "<p>x</p>".into();
        page.links = vec![
            ("  A link  ".into(), "https://a.example/".into()),
            ("b".repeat(90), "https://b.example/".into()),
        ];
    });
    let read = |arguments: Value| {
        let mut arguments = arguments;
        arguments["tabId"] = json!("mine");
        run(&browser, "read_page", arguments)
    };
    let head = "# Mine\nhttps://example.com/mine\n\n";
    assert_eq!(text(&read(json!({})).await), format!("{head}abcdef"));
    assert_eq!(
        text(&read(json!({ "maxChars": 3 })).await),
        format!("{head}abc")
    );
    assert_eq!(
        text(&read(json!({ "maxChars": 2.9 })).await),
        format!("{head}ab")
    );
    assert_eq!(
        text(&read(json!({ "maxChars": -2 })).await),
        format!("{head}abcd")
    );
    assert_eq!(text(&read(json!({ "maxChars": -0.5 })).await), head);
    assert_eq!(
        text(&read(json!({ "maxChars": 1e300 })).await),
        format!("{head}abcdef")
    );
    assert_eq!(
        text(&read(json!({ "format": "html" })).await),
        format!("{head}<p>x</p>")
    );
    assert_eq!(
        text(&read(json!({ "format": "links" })).await),
        format!(
            "{head}A link -> https://a.example/\n{} -> https://b.example/",
            "b".repeat(80)
        )
    );
    // Cuts count UTF-16 units; a pair cut in half is left out.
    browser.page(&mine, |page| page.text = "a😀b".into());
    assert_eq!(
        text(&read(json!({ "maxChars": 3 })).await),
        format!("{head}a😀")
    );
    assert_eq!(
        text(&read(json!({ "maxChars": 2 })).await),
        format!("{head}a")
    );
}

#[tokio::test]
async fn console_logs_filters_and_keeps_the_newest() {
    let (browser, mine, ..) = browser();
    let entry = |level, message: &str, source: Option<&str>| ConsoleEntry {
        level,
        message: message.into(),
        source: source.map(Into::into),
    };
    browser.page(&mine, |page| {
        page.console = vec![
            entry(ConsoleLevel::Info, "one", None),
            entry(ConsoleLevel::Error, "two", Some("app.js:3")),
            entry(ConsoleLevel::Info, "three", Some("")),
        ];
    });
    let logs = |arguments: Value| {
        let mut arguments = arguments;
        arguments["tabId"] = json!("mine");
        run(&browser, "console_logs", arguments)
    };
    assert_eq!(
        text(&logs(json!({})).await),
        "[info] one\n[error] two  (app.js:3)\n[info] three"
    );
    assert_eq!(
        text(&logs(json!({ "level": "info" })).await),
        "[info] one\n[info] three"
    );
    assert_eq!(text(&logs(json!({ "limit": 1 })).await), "[info] three");
    // `slice(-limit)`: 0 keeps all, a negative limit drops from the front, fractions truncate.
    assert_eq!(text(&logs(json!({ "limit": 0 })).await).lines().count(), 3);
    assert_eq!(
        text(&logs(json!({ "limit": -1 })).await),
        "[error] two  (app.js:3)\n[info] three"
    );
    assert_eq!(text(&logs(json!({ "limit": 1.9 })).await), "[info] three");
    assert_eq!(
        text(&logs(json!({ "level": "debug" })).await),
        "(no messages)"
    );
}

#[tokio::test]
async fn results_are_formatted_as_the_shell_formats_them() {
    let (browser, mine, ..) = browser();
    browser.page(&mine, |page| {
        page.elements.insert("#go".into());
        page.evaluations
            .insert("obj".into(), Evaluated::Value(json!({ "a": [1, 2] })));
        page.evaluations
            .insert("str".into(), Evaluated::Value(json!("plain")));
        page.evaluations
            .insert("nil".into(), Evaluated::Value(Value::Null));
    });
    let tool = |name: &'static str, arguments: Value| {
        let mut arguments = arguments;
        arguments["tabId"] = json!("mine");
        run(&browser, name, arguments)
    };
    assert_eq!(
        text(&tool("evaluate", json!({ "expression": "obj" })).await),
        "{\n  \"a\": [\n    1,\n    2\n  ]\n}"
    );
    assert_eq!(
        text(&tool("evaluate", json!({ "expression": "str" })).await),
        "plain"
    );
    assert_eq!(
        text(&tool("evaluate", json!({ "expression": "nil" })).await),
        "null"
    );
    assert_eq!(
        text(&tool("evaluate", json!({ "expression": "other" })).await),
        "undefined"
    );
    assert_eq!(
        text(&tool("click", json!({ "selector": "#go" })).await),
        "{\n  \"clicked\": \"#go\",\n  \"url\": \"https://example.com/mine\"\n}"
    );
    let missing = tool("click", json!({ "selector": "#gone" })).await;
    assert!(failed(&missing));
    assert_eq!(text(&missing), "No visible element matches #gone");
    let missing = tool("type", json!({ "selector": "#gone", "text": "x" })).await;
    assert_eq!(text(&missing), "No element matches #gone");
    assert_eq!(
        text(&tool("type", json!({ "text": "x", "submit": true })).await),
        "ok"
    );
    browser.page(&mine, |page| {
        assert_eq!(
            (page.typed.clone(), page.keys.clone()),
            (vec!["x".to_owned()], vec!["Enter".to_owned()])
        )
    });
    let navigated = tool("navigate", json!({ "url": "https://example.com/next" })).await;
    assert_eq!(
        text(&navigated),
        "{\n  \"url\": \"https://example.com/next\",\n  \"title\": \"https://example.com/next\"\n}"
    );
    // An empty url counts as none.
    let back = tool("navigate", json!({ "url": "", "action": "back" })).await;
    assert!(text(&back).contains("\"url\": \"https://example.com/mine\""));
    let shot = tool("screenshot", json!({})).await;
    let image = serde_json::to_value(&shot.content[0]).unwrap();
    assert_eq!(
        (image["type"].as_str(), image["mimeType"].as_str()),
        (Some("image"), Some("image/png"))
    );
    let unknown = run(&browser, "nope", json!({})).await;
    assert_eq!(
        (failed(&unknown), text(&unknown)),
        (true, "MCP error -32602: Tool nope not found")
    );
    // A success carries no isError, as from the TS SDK.
    let ok = serde_json::to_value(tool("press_key", json!({ "key": "Tab" })).await).unwrap();
    assert_eq!(ok, json!({ "content": [{ "type": "text", "text": "ok" }] }));
}

#[tokio::test]
async fn wait_for_polls_until_ready_or_its_timeout() {
    let (browser, mine, ..) = browser();
    let browser = Arc::new(browser);
    let wait = |arguments: Value| {
        let browser = browser.clone();
        let mut arguments = arguments;
        arguments["tabId"] = json!("mine");
        async move { run(&browser, "wait_for", arguments).await }
    };
    assert_eq!(text(&wait(json!({})).await), "ready");
    assert_eq!(
        text(&wait(json!({ "timeoutMs": 0 })).await),
        "Timed out after 0ms"
    );
    assert_eq!(
        text(&wait(json!({ "selector": "#late", "timeoutMs": 300 })).await),
        "Timed out after 300ms"
    );
    let pending = tokio::spawn(wait(json!({ "selector": "#late" })));
    tokio::time::sleep(Duration::from_millis(300)).await;
    browser.page(&mine, |page| {
        page.elements.insert("#late".into());
    });
    assert_eq!(text(&pending.await.unwrap()), "ready");
}

#[tokio::test]
async fn a_passed_deadline_drops_the_backend_call() {
    let (browser, mine, ..) = browser();
    browser.page(&mine, |page| page.hung = true);
    let result = run(
        &browser,
        "wait_for",
        json!({ "tabId": "mine", "timeoutMs": 200 }),
    )
    .await;
    assert_eq!(text(&result), "Timed out after 200ms");
    assert_eq!(browser.stats.dropped.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn a_hung_tool_fails_after_the_tool_timeout() {
    let (browser, mine, ..) = browser();
    browser.page(&mine, |page| page.hung = true);
    let result = run(
        &browser,
        "evaluate",
        json!({ "tabId": "mine", "expression": "1" }),
    )
    .await;
    assert_eq!(text(&result), "Flecha didn’t answer evaluate within 60s");
    assert_eq!(browser.stats.dropped.load(Ordering::Relaxed), 1);
    let result = run(
        &browser,
        "wait_for",
        json!({ "tabId": "mine", "timeoutMs": 4_000_000 }),
    )
    .await;
    assert_eq!(text(&result), "Timed out after 4000000ms");
}

#[tokio::test]
async fn a_client_that_disconnects_cancels_its_call() {
    let browser = Arc::new(browser().0);
    browser.page(&TabId::new("mine"), |page| page.hung = true);
    let run = start_with(browser.clone());
    let request = post(
        run.port(),
        Some(&run.bearer()),
        &[],
        &tool_call("wait_for", json!({ "tabId": "mine", "timeoutMs": 600_000 })),
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", run.port()))
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut stream, &request)
        .await
        .unwrap();
    while browser.stats.probes.load(Ordering::Relaxed) == 0 {
        tokio::task::yield_now().await;
    }
    drop(stream);
    let dropped = async {
        while browser.stats.dropped.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(flecha_services::agent::tools::TOOL_TIMEOUT, dropped)
        .await
        .expect("the backend call is dropped when the client goes away");
}

#[tokio::test]
async fn tools_run_over_http() {
    let run = start();
    let opened = call_tool(&run, "open_tab", json!({ "url": "https://example.org/" })).await;
    let reply: Value = serde_json::from_str(text_of(&opened)).unwrap();
    let tab = reply["tabId"].as_str().unwrap();
    let read = call_tool(&run, "read_page", json!({ "tabId": tab })).await;
    assert_eq!(
        text_of(&read),
        "# https://example.org/\nhttps://example.org/\n\n"
    );
    let invalid = call_tool(&run, "wait_for", json!({ "tabId": tab, "timeoutMs": -1 })).await;
    assert_eq!(invalid["isError"], true);
    assert_eq!(
        text_of(&invalid),
        "MCP error -32602: Input validation error: Invalid arguments for tool wait_for: Too small: expected number to be >=0 at timeoutMs"
    );
}

#[tokio::test]
async fn each_tool_tells_the_sidebar_what_it_does_in_the_shells_words() {
    let (browser, mine, ..) = browser();
    let cases = [
        ("list_tabs", json!({}), None, "listing tabs"),
        (
            "open_tab",
            json!({ "url": "example.org" }),
            None,
            "opening a tab",
        ),
        (
            "navigate",
            json!({ "url": "https://example.org/a" }),
            Some(&mine),
            "opening https://example.org/a",
        ),
        ("navigate", json!({ "action": "back" }), Some(&mine), "back"),
        ("read_page", json!({}), Some(&mine), "reading the page"),
        (
            "click",
            json!({ "selector": "#go" }),
            Some(&mine),
            "clicking #go",
        ),
        (
            "type",
            json!({ "text": "x" }),
            Some(&mine),
            "typing into the page",
        ),
        (
            "press_key",
            json!({ "key": "Enter" }),
            Some(&mine),
            "pressing Enter",
        ),
        (
            "evaluate",
            json!({ "expression": "1" }),
            Some(&mine),
            "running JavaScript",
        ),
        (
            "wait_for",
            json!({ "selector": "#ready", "timeoutMs": 0 }),
            Some(&mine),
            "waiting for #ready",
        ),
    ];
    for (tool, mut arguments, tab, action) in cases {
        if tab.is_some() {
            arguments["tabId"] = json!(mine.as_str());
        }
        run(&browser, tool, arguments).await;
        let last = browser.stats.activity.lock().unwrap().last().cloned();
        assert_eq!(last, Some((tab.cloned(), action.to_owned())), "{tool}");
    }
}
