use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};

use async_io::Async;
use flecha_core::{SearchEngine, SpaceId};
use futures::executor::block_on;
use futures::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use futures::try_join;

use super::*;

struct Fixture(Async<TcpListener>);

impl Fixture {
    fn new() -> Self {
        Self(Async::<TcpListener>::bind((Ipv4Addr::LOCALHOST, 0)).unwrap())
    }

    fn address(&self) -> SocketAddr {
        self.0.get_ref().local_addr().unwrap()
    }

    async fn request(&self, path: &str) -> io::Result<Async<TcpStream>> {
        let (stream, _) = self.0.accept().await?;
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await?;
        assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
        loop {
            line.clear();
            assert_ne!(stream.read_line(&mut line).await?, 0, "truncated headers");
            if line == "\r\n" {
                return Ok(stream.into_inner());
            }
            assert!(!line.to_ascii_lowercase().starts_with("cookie:"));
        }
    }
}

async fn respond(mut stream: Async<TcpStream>, charset: &str, body: &[u8]) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset={charset}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await
}

fn core() -> BrowserState {
    BrowserState::new(SpaceId::new("space"), WindowId::new("main"))
}

#[test]
fn fixture_suggestions_use_the_chosen_engine_and_keep_the_chosen_local_row() {
    let fixture = Fixture::new();
    let client = SuggestionClient::fixture();
    let mut core = core();
    core.settings.search_engine = SearchEngine::DuckDuckGo;
    let mut history = HistoryStore::in_memory().unwrap();
    history
        .record_visit("https://menu.example", "Mycafé", 1)
        .unwrap();
    history
        .record_visit("https://guide.example", "A café guide", 2)
        .unwrap();
    let mut matches = CommandMatches::new(
        Sources::regular(&core, Some(&history)),
        None,
        "  café  ",
        false,
    );
    matches.select(Key::Up);
    assert_eq!(matches.rows[matches.selected].label, "Mycafé");
    let request = matches.request.clone().unwrap();
    let terms = block_on(async {
        let (terms, ()) = try_join!(client.load_fixture(&request, fixture.address()), async {
            let stream = fixture.request("/ac/?q=caf%C3%A9&type=list").await?;
            respond(
                stream,
                "ISO-8859-1",
                b"[\"caf\xe9\",[\"caf\xe9 bar\",\"github.com\",\"Caf\xe9 Bar\"]]",
            )
            .await?;
            Ok(())
        })?;
        Ok::<_, anyhow::Error>(terms)
    })
    .unwrap();
    assert!(matches.complete(&request, terms));
    assert_eq!(
        matches
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["café", "A café guide", "café bar", "github.com", "Mycafé"]
    );
    assert_eq!(matches.rows[matches.selected].label, "Mycafé");
    assert_eq!(
        matches.rows[3].target,
        Target::Search("https://duckduckgo.com/?q=github.com".into())
    );
}

#[test]
fn private_inputs_and_the_off_setting_make_no_request() {
    let mut core = core();
    for input in [
        "https://host.example/private?token=x",
        "host.example",
        "localhost:8080",
        "about:blank",
        "user:pass@host.example",
        "user:pass@\u{feff}host.example",
        "user@\u{feff}\thost.example",
        "mdn user:pass@\u{feff}host.example",
        "notes example.com/private/..",
        "/home/me/private",
    ] {
        let model = CommandMatches::new(Sources::regular(&core, None), None, input, true);
        assert!(model.request.is_none(), "{input:?}");
    }

    core.settings.search_suggest_enabled = Some(false);
    let model = CommandMatches::new(Sources::regular(&core, None), None, "weather", false);
    assert!(model.request.is_none());
    assert_eq!(model.rows[0].label, "weather");
}

#[test]
fn an_older_fixture_answer_is_dropped_even_when_it_arrives_last() {
    let fixture = Fixture::new();
    let client = SuggestionClient::fixture();
    let core = core();
    let mut matches = CommandMatches::new(Sources::regular(&core, None), None, "wea", false);
    let old = matches.request.clone().unwrap();
    let old_answer = block_on(async {
        let (old_answer, ()) = try_join!(client.load_fixture(&old, fixture.address()), async {
            let old_stream = fixture
                .request("/complete/search?client=chrome-omni&q=wea")
                .await?;
            matches = CommandMatches::new(Sources::regular(&core, None), None, "weat", false);
            let current = matches.request.clone().unwrap();
            let (terms, ()) = try_join!(client.load_fixture(&current, fixture.address()), async {
                let stream = fixture
                    .request("/complete/search?client=chrome-omni&q=weat")
                    .await?;
                respond(stream, "utf-8", br#"["weat",["weather"]]"#).await?;
                Ok(())
            })?;
            assert!(matches.complete(&current, terms));
            respond(old_stream, "utf-8", br#"["wea",["wealth"]]"#).await?;
            Ok(())
        })?;
        Ok::<_, anyhow::Error>(old_answer)
    })
    .unwrap();
    assert_eq!(old_answer, ["wealth"]);
    assert!(!matches.complete(&old, old_answer));
    assert_eq!(
        matches
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["weat", "weather"]
    );
}

#[test]
fn a_failed_fixture_request_keeps_local_results() {
    let fixture = Fixture::new();
    let client = SuggestionClient::fixture();
    let core = core();
    let mut history = HistoryStore::in_memory().unwrap();
    history
        .record_visit("https://weather.example", "Local weather", 1)
        .unwrap();
    let mut matches = CommandMatches::new(
        Sources::regular(&core, Some(&history)),
        None,
        "weather",
        false,
    );
    let request = matches.request.clone().unwrap();
    let result = block_on(async {
        futures::join!(client.load_fixture(&request, fixture.address()), async {
            drop(
                fixture
                    .request("/complete/search?client=chrome-omni&q=weather")
                    .await
                    .unwrap(),
            );
        })
        .0
    });
    assert!(result.is_err());
    assert!(matches.complete(&request, result.unwrap_or_default()));
    assert_eq!(
        matches
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["weather", "Local weather"]
    );
}

#[test]
fn an_incognito_window_asks_no_search_provider() {
    let profile = tempfile::tempdir().unwrap();
    let mut controller = Controller::open_test_profile(profile.path()).unwrap();
    let regular = controller.model.core.focused_window().id.clone();
    controller
        .act(&regular, Action::Open("https://regular.example/".into()))
        .unwrap();
    controller
        .act(&regular, Action::NewIncognitoWindow)
        .unwrap();
    let private = controller.model.incognito_windows()[0].id.clone();
    controller
        .act(&private, Action::Open("https://private.example/".into()))
        .unwrap();
    // The command bar leaves out the tab it is on.
    controller
        .act(&private, Action::Open("https://current.example/".into()))
        .unwrap();
    assert!(controller.model.core.settings.search_suggestions_enabled());

    let matches = |window: &WindowId, text: &str| {
        let sources = controller.command_sources(window).unwrap();
        CommandMatches::new(sources, controller.model.active(window), text, true)
    };
    assert!(matches(&regular, "weather").request.is_some());
    let private_matches = matches(&private, "weather");
    assert!(private_matches.request.is_none());
    assert_eq!(private_matches.rows[0].label, "weather");

    // Each window finds its own tabs.
    let tabs = |window: &WindowId, text: &str| {
        matches(window, text)
            .rows
            .into_iter()
            .filter(|row| matches!(row.target, Target::Tab(..)))
            .map(|row| row.target)
            .collect::<Vec<_>>()
    };
    assert!(tabs(&regular, "private").is_empty());
    assert!(tabs(&private, "regular").is_empty());
    assert_eq!(tabs(&private, "private").len(), 1);
}
