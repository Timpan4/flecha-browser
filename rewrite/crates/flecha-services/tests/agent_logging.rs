//! Nothing the server logs carries the token, page contents or addresses, even with every
//! target enabled at TRACE. Its own process, so the capturing subscriber can be the global one.

mod common;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use common::*;
use flecha_core::TabId;
use flecha_services::agent::Evaluated;
use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Metadata, Subscriber};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<String>>);

impl Visit for Capture {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0.lock().unwrap(), "{}={value:?} ", field.name());
    }
}

impl Subscriber for Capture {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, span: &Attributes<'_>) -> Id {
        span.record(&mut self.clone());
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, values: &Record<'_>) {
        values.record(&mut self.clone());
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let _ = write!(
            self.0.lock().unwrap(),
            "[{} {}] ",
            event.metadata().level(),
            event.metadata().target()
        );
        event.record(&mut self.clone());
        self.0.lock().unwrap().push('\n');
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test]
async fn logs_carry_no_token_page_contents_or_addresses() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(capture.clone()).unwrap();
    tracing::trace!(target: "rmcp::probe", "capture works");
    assert!(
        capture.0.lock().unwrap().contains("capture works"),
        "the subscriber records rmcp TRACE events"
    );

    let run = start();
    let opened = call_tool(
        &run,
        "open_tab",
        json!({ "url": "https://secret-host.example/private?q=SENTINEL_URL" }),
    )
    .await;
    let tab: Value = serde_json::from_str(text_of(&opened)).unwrap();
    let tab = tab["tabId"].as_str().unwrap().to_owned();
    run.browser.page(&TabId::new(tab.as_str()), |page| {
        page.text = "SENTINEL_PAGE_TEXT".into();
        page.html = "<p>SENTINEL_PAGE_HTML</p>".into();
        page.evaluations
            .insert("secret()".into(), Evaluated::Value(json!("SENTINEL_EVAL")));
    });
    call_tool(&run, "list_tabs", json!({})).await;
    call_tool(&run, "read_page", json!({ "tabId": tab })).await;
    call_tool(&run, "read_page", json!({ "tabId": tab, "format": "html" })).await;
    call_tool(
        &run,
        "evaluate",
        json!({ "tabId": tab, "expression": "secret()" }),
    )
    .await;
    call_tool(
        &run,
        "navigate",
        json!({ "tabId": tab, "url": "https://secret-host.example/next?q=SENTINEL_URL" }),
    )
    .await;
    call_tool(
        &run,
        "type",
        json!({ "tabId": tab, "text": "SENTINEL_TYPED" }),
    )
    .await;
    let initialize = rpc(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
    );
    exchange(
        run.port(),
        &post(run.port(), Some(&run.bearer()), &[], &initialize),
    )
    .await;
    exchange(
        run.port(),
        &post(
            run.port(),
            Some("Bearer SENTINEL_WRONG_TOKEN"),
            &[],
            &initialize,
        ),
    )
    .await;
    let token = run.token.reveal();
    let port = run.port();
    drop(run);

    let logs = capture.0.lock().unwrap().clone();
    for secret in [
        token.as_str(),
        "SENTINEL_URL",
        "secret-host.example",
        "SENTINEL_PAGE_TEXT",
        "SENTINEL_PAGE_HTML",
        "SENTINEL_EVAL",
        "SENTINEL_TYPED",
        "SENTINEL_WRONG_TOKEN",
        "secret()",
    ] {
        assert!(!logs.contains(secret), "logs contain {secret:?}:\n{logs}");
    }
    eprintln!(
        "captured {} log lines from a session on port {port}",
        logs.lines().count()
    );
}
