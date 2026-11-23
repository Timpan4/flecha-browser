//! The trust boundary, one rule per test. Each refusal is sent with everything else valid, so
//! it fails for the rule under test and not an earlier one.

mod common;

use std::net::{Ipv4Addr, TcpStream, UdpSocket};
use std::time::Duration;

use common::*;
use flecha_services::agent::AgentToken;
use serde_json::json;

const LIST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;

/// The SDK default body bound: `DEFAULT_MAX_REQUEST_BODY_BYTES` in rmcp 3.11.0, the same 4 MB
/// as `MAXIMUM_MESSAGE_SIZE` in the TS SDK.
const BODY_LIMIT: usize = 4 * 1024 * 1024;

#[tokio::test]
async fn a_valid_request_lists_the_tools() {
    let run = start();
    let reply = exchange(
        run.port(),
        &post(run.port(), Some(&run.bearer()), &[], LIST),
    )
    .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("content-type"), Some("text/event-stream"));
    assert_eq!(
        reply.message()["result"]["tools"].as_array().unwrap().len(),
        13
    );
}

#[tokio::test]
async fn refuses_a_request_without_the_token() {
    let run = start();
    let reply = exchange(run.port(), &post(run.port(), None, &[], LIST)).await;
    assert_eq!(
        (reply.status, reply.body.as_str()),
        (401, "Missing or wrong token")
    );
}

#[tokio::test]
async fn refuses_a_wrong_token() {
    let run = start();
    let token = run.token.reveal();
    let mut flipped = token.clone().into_bytes();
    flipped[0] = if flipped[0] == b'A' { b'B' } else { b'A' };
    let wrong = [
        format!("Bearer {}", String::from_utf8(flipped).unwrap()),
        format!("Bearer {token}x"),
        format!("Bearer {}", &token[..token.len() - 1]),
        format!("bearer {token}"),
        format!("Bearer  {token}"),
        token.clone(),
        "Bearer ".to_owned(),
        String::new(),
    ];
    for authorization in wrong {
        let reply = exchange(
            run.port(),
            &post(run.port(), Some(&authorization), &[], LIST),
        )
        .await;
        assert_eq!(reply.status, 401, "{authorization:?}");
    }
}

#[tokio::test]
async fn refuses_a_repeated_authorization_header() {
    let run = start();
    let bearer = run.bearer();
    let reply = exchange(
        run.port(),
        &post(
            run.port(),
            Some(&bearer),
            &[("Authorization", &bearer)],
            LIST,
        ),
    )
    .await;
    assert_eq!(reply.status, 401);
}

#[tokio::test]
async fn a_renewed_token_takes_effect_on_the_next_request() {
    let run = start();
    let old = run.bearer();
    run.token.renew().unwrap();
    let new = run.bearer();
    assert_ne!(old, new);
    let reply = exchange(run.port(), &post(run.port(), Some(&old), &[], LIST)).await;
    assert_eq!(reply.status, 401);
    let reply = exchange(run.port(), &post(run.port(), Some(&new), &[], LIST)).await;
    assert_eq!(reply.status, 200);
    // The file holds the new token, so a restart keeps it.
    let reopened = AgentToken::open(run.dir.path.join("agent-token")).unwrap();
    assert_eq!(format!("Bearer {}", reopened.reveal()), new);
}

#[tokio::test]
async fn refuses_a_foreign_host_even_with_the_token() {
    let run = start();
    let port = run.port().to_string();
    for host in [
        format!("evil.example:{port}"),
        format!("localhost.evil.example:{port}"),
        format!("127.0.0.1.evil.example:{port}"),
        "evil.example".to_owned(),
        format!("[::1]:{port}"),
        format!("0.0.0.0:{port}"),
        format!("LOCALHOST:{port}"),
        "localhost:".to_owned(),
    ] {
        let request = String::from_utf8(post(run.port(), Some(&run.bearer()), &[], LIST))
            .unwrap()
            .replace(&format!("Host: 127.0.0.1:{port}"), &format!("Host: {host}"));
        let reply = exchange(run.port(), request.as_bytes()).await;
        assert_eq!(
            (reply.status, reply.body.as_str()),
            (403, "Forbidden host"),
            "{host}"
        );
    }
    for host in [
        format!("localhost:{port}"),
        "127.0.0.1".to_owned(),
        "localhost".to_owned(),
    ] {
        let request = String::from_utf8(post(run.port(), Some(&run.bearer()), &[], LIST))
            .unwrap()
            .replace(&format!("Host: 127.0.0.1:{port}"), &format!("Host: {host}"));
        assert_eq!(
            exchange(run.port(), request.as_bytes()).await.status,
            200,
            "{host}"
        );
    }
}

#[tokio::test]
async fn refuses_a_missing_or_repeated_host() {
    let run = start();
    let port = run.port();
    let base = String::from_utf8(post(port, Some(&run.bearer()), &[], LIST)).unwrap();
    let without = base.replace(&format!("Host: 127.0.0.1:{port}\r\n"), "");
    // HTTP/1.0 may leave Host out; HTTP/1.1 parsers reject it outright.
    let http10 = without.replacen("HTTP/1.1", "HTTP/1.0", 1);
    let reply = exchange(port, http10.as_bytes()).await;
    assert_eq!(reply.status, 403, "{reply:?}");
    let repeated = base.replace("\r\n\r\n", "\r\nHost: evil.example\r\n\r\n");
    let reply = exchange(port, repeated.as_bytes()).await;
    assert!(matches!(reply.status, 400 | 403), "{reply:?}");
    let repeated_local = base.replace("\r\n\r\n", &format!("\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    let reply = exchange(port, repeated_local.as_bytes()).await;
    assert!(matches!(reply.status, 400 | 403), "{reply:?}");
}

#[tokio::test]
async fn refuses_an_absolute_uri_naming_another_host() {
    let run = start();
    let port = run.port();
    let request = String::from_utf8(post(port, Some(&run.bearer()), &[], LIST))
        .unwrap()
        .replacen("POST /mcp", "POST http://evil.example/mcp", 1);
    let reply = exchange(port, request.as_bytes()).await;
    assert_eq!(reply.status, 403, "{reply:?}");
}

#[tokio::test]
async fn refuses_a_foreign_origin_even_with_the_token() {
    let run = start();
    let port = run.port();
    for origin in [
        "https://evil.example".to_owned(),
        format!("http://evil.example:{port}"),
        "null".to_owned(),
        format!("http://127.0.0.1:{}", port.wrapping_add(1)),
        format!("https://127.0.0.1:{port}"),
        format!("http://localhost.evil.example:{port}"),
    ] {
        let reply = exchange(
            port,
            &post(port, Some(&run.bearer()), &[("Origin", &origin)], LIST),
        )
        .await;
        assert_eq!(
            (reply.status, reply.body.as_str()),
            (403, "Forbidden origin"),
            "{origin}"
        );
    }
    let own = format!("http://127.0.0.1:{port}");
    let reply = exchange(
        port,
        &post(
            port,
            Some(&run.bearer()),
            &[("Origin", &own), ("Origin", &own)],
            LIST,
        ),
    )
    .await;
    assert_eq!(reply.status, 403);
    for origin in [own, format!("http://localhost:{port}")] {
        let reply = exchange(
            port,
            &post(port, Some(&run.bearer()), &[("Origin", &origin)], LIST),
        )
        .await;
        assert_eq!(reply.status, 200, "{origin}");
    }
}

#[tokio::test]
async fn refuses_an_oversized_body_with_a_content_length() {
    let run = start();
    let port = run.port();
    let padding = "x".repeat(BODY_LIMIT);
    let body = tool_call("open_tab", json!({ "url": padding }));
    assert!(body.len() > BODY_LIMIT);
    let reply = exchange(port, &post(port, Some(&run.bearer()), &[], &body)).await;
    assert_eq!(reply.status, 413, "{}", reply.body);
    // Just under the bound is served.
    let fits = tool_call("open_tab", json!({ "url": "x".repeat(BODY_LIMIT - 200) }));
    assert!(fits.len() <= BODY_LIMIT);
    let reply = exchange(port, &post(port, Some(&run.bearer()), &[], &fits)).await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn refuses_an_oversized_chunked_body() {
    let run = start();
    let port = run.port();
    let body = tool_call("open_tab", json!({ "url": "x".repeat(BODY_LIMIT) }));
    let mut request = String::from_utf8(post(port, Some(&run.bearer()), &[], ""))
        .unwrap()
        .replace("Content-Length: 0\r\n", "Transfer-Encoding: chunked\r\n");
    for chunk in body.as_bytes().chunks(64 * 1024) {
        request.push_str(&format!(
            "{:x}\r\n{}\r\n",
            chunk.len(),
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    request.push_str("0\r\n\r\n");
    let reply = exchange(port, request.as_bytes()).await;
    assert_eq!(reply.status, 413, "{}", reply.body);
}

#[tokio::test]
async fn an_unauthorized_body_is_refused_before_it_is_read() {
    let run = start();
    let port = run.port();
    let body = "x".repeat(BODY_LIMIT * 2);
    let reply = exchange(port, &post(port, None, &[], &body)).await;
    assert_eq!(reply.status, 401);
}

#[tokio::test]
async fn refuses_a_malformed_body() {
    let run = start();
    let port = run.port();
    // rmcp 3.11.0 answers any body it cannot read as a JSON-RPC message with 415
    // (`expect_json` in transport/common/server_side_http.rs); the TS SDK answered 400.
    for body in [
        "{",
        "not json",
        "[]",
        r#"{"jsonrpc":"2.0","id":1}"#,
        "\u{0}",
        "",
    ] {
        let reply = exchange(port, &post(port, Some(&run.bearer()), &[], body)).await;
        assert_eq!(reply.status, 415, "{body:?}: {reply:?}");
        assert!(
            reply.body.starts_with("fail to deserialize request body"),
            "{reply:?}"
        );
    }
    // Arguments that are not an object are a JSON-RPC error, not a tool call.
    let reply = exchange(
        port,
        &post(
            port,
            Some(&run.bearer()),
            &[],
            &tool_call("open_tab", json!("x")),
        ),
    )
    .await;
    assert!(
        reply.status == 400 || reply.message().get("error").is_some(),
        "{reply:?}"
    );
    assert!(run.browser.state().open_tabs().is_empty());
}

#[tokio::test]
async fn serves_only_the_mcp_path_and_post() {
    let run = start();
    let port = run.port();
    for path in ["/", "/mcpx", "/mcp/", "/other/mcp"] {
        let request = String::from_utf8(post(port, Some(&run.bearer()), &[], LIST))
            .unwrap()
            .replacen("POST /mcp ", &format!("POST {path} "), 1);
        assert_eq!(
            exchange(port, request.as_bytes()).await.status,
            404,
            "{path}"
        );
    }
    let get = format!(
        "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\nAuthorization: {}\r\nConnection: close\r\n\r\n",
        run.bearer()
    );
    assert_eq!(exchange(port, get.as_bytes()).await.status, 405);
}

#[test]
fn listens_on_loopback_only() {
    let run = start();
    assert_eq!(
        run.server.endpoint(),
        format!("http://127.0.0.1:{}/mcp", run.port())
    );
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, run.port())).is_ok());
    // This host's address on its default route, if it has one: nothing listens there.
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let outward = probe
        .connect((Ipv4Addr::new(192, 0, 2, 1), 9))
        .and_then(|()| probe.local_addr())
        .map(|addr| addr.ip())
        .ok()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
    match outward {
        Some(ip) => {
            let refused =
                TcpStream::connect_timeout(&(ip, run.port()).into(), Duration::from_secs(5));
            assert!(refused.is_err(), "reachable on {ip}");
        }
        None => eprintln!("no non-loopback address on this host; the bind test covers the rule"),
    }
}

#[cfg(unix)]
mod token_file {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use super::*;

    #[test]
    fn is_owner_only_and_well_formed() {
        let dir = TempDir::new();
        let path = dir.path.join("agent-token");
        let token = AgentToken::open(&path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(saved, token.reveal());
        assert_eq!(saved.len(), 32);
        assert!(
            saved
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        // Reopening keeps it.
        assert_eq!(AgentToken::open(&path).unwrap().reveal(), saved);
        token.renew().unwrap();
        assert_ne!(token.reveal(), saved);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // No temporary file is left beside it.
        assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_renewals_leave_the_file_and_the_server_agreeing() {
        let dir = TempDir::new();
        let path = dir.path.join("agent-token");
        let token = AgentToken::open(&path).unwrap();
        let workers = std::thread::available_parallelism().unwrap().get();
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| token.renew().unwrap());
            }
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), token.reveal());
        assert_eq!(std::fs::read_dir(&dir.path).unwrap().count(), 1);
    }

    #[test]
    fn replaces_a_token_others_could_read() {
        let dir = TempDir::new();
        let path = dir.path.join("agent-token");
        let leaked = "A".repeat(32);
        for mode in [0o644, 0o640, 0o604, 0o660, 0o602] {
            std::fs::write(&path, &leaked).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let token = AgentToken::open(&path).unwrap();
            assert_ne!(token.reveal(), leaked, "mode {mode:o}");
            assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn replaces_a_malformed_token_or_a_symlink() {
        let dir = TempDir::new();
        let path = dir.path.join("agent-token");
        for bad in [
            "",
            "short",
            &"A".repeat(33),
            &"!".repeat(32),
            &format!("{}\n", "A".repeat(32)),
        ] {
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, bad).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert_ne!(AgentToken::open(&path).unwrap().reveal(), bad);
        }
        let target = dir.path.join("elsewhere");
        let planted = "B".repeat(32);
        std::fs::write(&target, &planted).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        let token = AgentToken::open(&path).unwrap();
        assert_ne!(token.reveal(), planted);
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
        // The link's target is left alone.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), planted);
    }

    #[test]
    fn debug_output_hides_the_token() {
        let dir = TempDir::new();
        let token = AgentToken::open(dir.path.join("agent-token")).unwrap();
        assert!(!format!("{token:?}").contains(&token.reveal()));
    }
}
