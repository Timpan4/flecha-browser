//! The agent server over the fake backend, for end-to-end checks with a real MCP client.
//!
//!     fake_agent_server <token-file>
//!
//! Prints the endpoint on stdout, serves until stdin closes, then stops. It listens on an
//! ephemeral port, never on Flecha's 7390–7399.

use std::io::{BufRead, Write};
use std::sync::Arc;

use flecha_core::TabId;
use flecha_services::agent::fake::FakeBrowser;
use flecha_services::agent::{AgentServer, AgentToken, Evaluated};
use serde_json::json;

fn main() -> std::io::Result<()> {
    let token_path = std::env::args_os()
        .nth(1)
        .expect("usage: fake_agent_server <token-file>");
    let browser = Arc::new(FakeBrowser::default());
    {
        let mut state = browser.state();
        let window = state.focused_window().id.clone();
        state.open_today(
            TabId::new("user-tab"),
            flecha_core::Tab::new("https://example.com/", 1),
            &window,
            None,
            true,
        );
    }
    browser.page(&TabId::new("user-tab"), |page| {
        page.title = "Example".into();
        page.text = "Example Domain".into();
        page.elements.insert("h1".into());
        page.evaluations
            .insert("document.title".into(), Evaluated::Value(json!("Example")));
        page.evaluations
            .insert("[1, 2]".into(), Evaluated::Value(json!([1, 2])));
    });
    let token = Arc::new(AgentToken::open(token_path)?);
    let server = AgentServer::start(browser, token, 0)?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", server.endpoint())?;
    stdout.flush()?;
    for line in std::io::stdin().lock().lines() {
        line?;
    }
    server.stop();
    Ok(())
}
