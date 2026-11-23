//! The real ureq transport against a local HTTP server. No network.

mod common;

use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use common::subscription;
use flecha_adblock::{
    DOWNLOAD_TIMEOUT, Download, DownloadRequest, LISTS, UreqDownload, WireResponse,
    fetch::{ListWarning, fetch_lists},
};

type Handler = dyn Fn(&str, &mut TcpStream) + Send + Sync;

/// Serves one request per connection on 127.0.0.1 and records each path.
fn server(handler: Arc<Handler>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let path = request_line
                .split(' ')
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            seen.lock().unwrap().push(path.clone());
            handler(&path, &mut stream);
        }
    });
    (port, requests)
}

fn reply(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
}

/// Routes the lists' HTTPS URLs to the local server without relaxing redirect checks, and
/// records each request's deadline.
struct Local {
    port: u16,
    deadlines: Mutex<HashMap<String, Instant>>,
    inner: UreqDownload,
}

impl Download for Local {
    fn get(&self, url: &str, request: &DownloadRequest) -> io::Result<WireResponse> {
        let path = url::Url::parse(url).unwrap().path().to_owned();
        self.deadlines
            .lock()
            .unwrap()
            .insert(path.clone(), request.deadline);
        self.inner
            .get(&format!("http://127.0.0.1:{}{path}", self.port), request)
    }
}

#[test]
fn a_relative_redirect_reaches_the_server_and_a_partial_refresh_keeps_the_cache() {
    let original = subscription(LISTS[0].url, "/cached-rule");
    let truncate = Arc::new(AtomicBool::new(false));
    let handler: Arc<Handler> = Arc::new({
        let original = original.clone();
        let truncate = Arc::clone(&truncate);
        move |path, stream| match path {
            "/easylist/easylist.txt" => {
                reply(
                    stream,
                    "302 Found",
                    &[("Location", "../updates/easylist.txt".into())],
                    b"",
                );
            }
            "/updates/easylist.txt" if truncate.load(Ordering::SeqCst) => {
                let partial = subscription(LISTS[0].url, "/partial-rule");
                let complete = format!("{partial}/another-rule\n");
                let length = complete.len().to_string();
                reply(
                    stream,
                    "200 OK",
                    &[("Content-Length", length)],
                    partial.as_bytes(),
                );
            }
            _ => {
                let body = if path == "/updates/easylist.txt" {
                    original.clone()
                } else {
                    subscription(LISTS[1].url, "/privacy")
                };
                reply(
                    stream,
                    "200 OK",
                    &[("Content-Length", body.len().to_string())],
                    body.as_bytes(),
                );
            }
        }
    });
    let (port, requests) = server(handler);
    let download = Local {
        port,
        deadlines: Mutex::default(),
        inner: UreqDownload::default(),
    };
    let cache = tempfile::tempdir().unwrap();
    let report = fetch_lists(cache.path(), false, &download);
    assert!(report.lists.is_ok(), "{report:?}");
    let file = cache.path().join("easylist.txt");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    let deadline = |path: &str| download.deadlines.lock().unwrap()[path];
    assert_eq!(
        deadline("/easylist/easylist.txt"),
        deadline("/updates/easylist.txt")
    );

    truncate.store(true, Ordering::SeqCst);
    let report = fetch_lists(cache.path(), true, &download);
    assert!(report.lists.is_ok(), "{report:?}");
    let easylist: Vec<_> = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.ends_with("/easylist.txt"))
        .cloned()
        .collect();
    assert_eq!(
        easylist,
        [
            "/easylist/easylist.txt",
            "/updates/easylist.txt",
            "/easylist/easylist.txt",
            "/updates/easylist.txt"
        ]
    );
    assert_eq!(
        deadline("/easylist/easylist.txt"),
        deadline("/updates/easylist.txt")
    );
    assert!(
        matches!(
            report.warnings.as_slice(),
            [ListWarning::UsedCache { id: "easylist", .. }]
        ),
        "{:?}",
        report.warnings
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
}

#[test]
fn ureq_hands_over_the_compressed_wire_bytes() {
    let text = subscription(LISTS[0].url, "/gzip-rule");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(text.as_bytes()).unwrap();
    let gzipped = encoder.finish().unwrap();
    let handler: Arc<Handler> = Arc::new({
        let gzipped = gzipped.clone();
        move |_, stream| {
            let headers = [
                ("Content-Encoding", "gzip".to_owned()),
                ("Content-Length", gzipped.len().to_string()),
            ];
            reply(stream, "200 OK", &headers, &gzipped);
        }
    });
    let (port, _) = server(handler);
    let request = DownloadRequest {
        accept_encoding: "gzip",
        deadline: Instant::now() + DOWNLOAD_TIMEOUT,
    };
    let mut response = UreqDownload::default()
        .get(
            &format!("http://127.0.0.1:{port}/easylist/easylist.txt"),
            &request,
        )
        .unwrap();
    // With ureq's gzip feature on, it would strip both headers and decode the body.
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(
        response.header("content-length"),
        Some(gzipped.len().to_string().as_str())
    );
    let mut body = Vec::new();
    response.body.read_to_end(&mut body).unwrap();
    assert_eq!(body, gzipped);
}
