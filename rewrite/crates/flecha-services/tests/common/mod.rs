//! A raw HTTP/1.1 client, so tests control every header and body byte the server sees.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flecha_services::agent::fake::FakeBrowser;
use flecha_services::agent::{AgentServer, AgentToken};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct Running {
    pub server: AgentServer,
    pub browser: Arc<FakeBrowser>,
    pub token: Arc<AgentToken>,
    pub dir: TempDir,
}

impl Running {
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.token.reveal())
    }
}

/// A server on an ephemeral port with a fresh token, never the owner's 7390–7399.
pub fn start() -> Running {
    start_with(Arc::new(FakeBrowser::default()))
}

pub fn start_with(browser: Arc<FakeBrowser>) -> Running {
    let dir = TempDir::new();
    let token = Arc::new(AgentToken::open(dir.path.join("agent-token")).unwrap());
    let server = AgentServer::start(browser.clone(), token.clone(), 0).unwrap();
    Running {
        server,
        browser,
        token,
        dir,
    }
}

pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "flecha-services-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    /// The JSON-RPC message in the body, whether sent as JSON or as one SSE `data:` line.
    pub fn message(&self) -> Value {
        let data = self
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .find(|data| !data.is_empty())
            .unwrap_or(&self.body);
        serde_json::from_str(data).unwrap_or_else(|err| panic!("{err}: {}", self.body))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Sends raw bytes and reads the reply until the server closes the connection.
pub async fn exchange(port: u16, request: &[u8]) -> Reply {
    exchange_on(connect(port).await, request).await
}

pub async fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).await.unwrap()
}

/// [`exchange`] on a connection the caller opened.
pub async fn exchange_on(stream: TcpStream, request: &[u8]) -> Reply {
    let (mut reader, mut writer) = stream.into_split();
    let request = request.to_vec();
    // The server may answer and close before it reads a large body, so read while writing.
    let writing = tokio::spawn(async move {
        let _ = writer.write_all(&request).await;
        writer
    });
    let mut raw = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
        }
    }
    drop(writing.await);
    parse(&raw)
}

pub fn parse(raw: &[u8]) -> Reply {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    let chunked = headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("transfer-encoding") && value.contains("chunked")
    });
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_owned()
    };
    Reply {
        status,
        headers,
        body,
    }
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || rest.len() < size {
            break;
        }
        out.push_str(&rest[..size]);
        body = rest[size..].strip_prefix("\r\n").unwrap_or(&rest[size..]);
    }
    out
}

/// A POST to `/mcp` with the headers an MCP client sends, plus `extra`. `None` leaves the
/// `Authorization` header out.
pub fn post(port: u16, authorization: Option<&str>, extra: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\n\
         Connection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(authorization) = authorization {
        request.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    for (name, value) in extra {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    request.into_bytes()
}

pub fn rpc(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string()
}

pub fn tool_call(name: &str, arguments: Value) -> String {
    rpc(
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

/// Calls a tool over HTTP and returns the `result` member.
pub async fn call_tool(run: &Running, name: &str, arguments: Value) -> Value {
    let reply = exchange(
        run.port(),
        &post(
            run.port(),
            Some(&run.bearer()),
            &[],
            &tool_call(name, arguments),
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    reply.message()["result"].clone()
}

/// The text of a tool result's first content item.
pub fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}
