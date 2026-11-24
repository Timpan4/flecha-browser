//! Dev servers on this machine, for the sidebar's "Running locally" and the empty page, as
//! `chromium/shell/features/localServers.ts` finds them: the ports dev tools use by default that
//! answer HTTP, each named after its page's `<title>`, or `localhost:port`.
//!
//! The shell knocked on every port each scan because an extension can't see sockets. Here the
//! kernel's socket tables (`/proc/net/tcp` and `/proc/net/tcp6`) say which of those ports listen
//! on a loopback or wildcard address, and only those are asked for a page. A port keeps its name
//! while it listens, as the shell kept it while it answered.
//!
//! The scan runs on a thread of its own every [`POLL`]; the UI takes the list when it changes.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io::{self, Read as _, Write as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `DEV_PORTS` in `localServers.ts`: default ports of common dev servers (Next, Rails, Express,
/// Vite, Astro, Angular, Storybook, Django, Hugo…).
pub const DEV_PORTS: [u16; 44] = [
    1234, 1313, 2368, 3000, 3001, 3002, 3003, 3004, 3005, 3030, 3333, 4000, 4173, 4200, 4321, 4400,
    4567, 5000, 5001, 5005, 5173, 5174, 5175, 5176, 5500, 6006, 7000, 7777, 8000, 8001, 8008, 8010,
    8080, 8081, 8082, 8088, 8443, 8787, 8888, 9000, 9001, 9090, 19006, 24678,
];

/// `FOCUSED_EVERY_MS` in `localServers.ts`.
pub const POLL: Duration = Duration::from_millis(4000);
/// `AbortSignal.timeout(1500)` on the shell's request for a page's title.
const PAGE_TIMEOUT: Duration = Duration::from_millis(1500);
/// `text.slice(0, 20000)`: how much of a page the shell read for its title.
const PAGE_BYTES: usize = 20_000;
/// `decoded.slice(0, 60)`: the longest name the shell gave a server.
const NAME_CHARS: usize = 60;
/// `st` in the kernel's socket table for a listening socket (`TCP_LISTEN`).
const LISTEN: &str = "0A";

/// A dev server, as `LocalServer` in `chromium/shared/types.d.ts` has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalServer {
    pub port: u16,
    pub url: String,
    pub name: String,
}

/// The loopback address that reaches a listening socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    fn loopback(self) -> IpAddr {
        match self {
            Self::V4 => Ipv4Addr::LOCALHOST.into(),
            Self::V6 => Ipv6Addr::LOCALHOST.into(),
        }
    }
}

/// The ports in one of the kernel's TCP tables (`/proc/net/tcp` or `/proc/net/tcp6`) that listen
/// where `localhost` reaches them: a loopback address or every address.
pub fn listening(table: &str) -> BTreeMap<u16, Family> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local = fields.nth(1)?;
            let state = fields.nth(1)?;
            if state != LISTEN {
                return None;
            }
            let (address, port) = local.split_once(':')?;
            let port = u16::from_str_radix(port, 16).ok()?;
            // A socket on ::ffff:127.0.0.1 takes IPv4 connections only.
            let (ip, family) = match table_address(address)? {
                (IpAddr::V6(v6), family) => v6
                    .to_ipv4_mapped()
                    .map_or((IpAddr::V6(v6), family), |v4| (v4.into(), Family::V4)),
                v4 => v4,
            };
            (ip.is_loopback() || ip.is_unspecified()).then_some((port, family))
        })
        .collect()
}

/// An address as the kernel prints it: the address's 32-bit words in hex, each in the host's
/// byte order.
fn table_address(hex: &str) -> Option<(IpAddr, Family)> {
    let words = (0..hex.len())
        .step_by(8)
        .map(|at| {
            let word = hex.get(at..at + 8)?;
            Some(u32::from_str_radix(word, 16).ok()?.to_ne_bytes())
        })
        .collect::<Option<Vec<_>>>()?;
    match words.as_slice() {
        [word] => Some((Ipv4Addr::from(*word).into(), Family::V4)),
        [a, b, c, d] => {
            let bytes: [u8; 16] = [*a, *b, *c, *d].concat().try_into().ok()?;
            Some((Ipv6Addr::from(bytes).into(), Family::V6))
        }
        _ => None,
    }
}

/// This machine's listening dev ports, read from both TCP tables. A table that can't be read
/// counts as empty, as on a kernel built without IPv6.
pub fn listening_dev_ports() -> BTreeMap<u16, Family> {
    let mut ports = BTreeMap::new();
    // IPv4 last, so a port listening on both is asked over 127.0.0.1 as the shell asked it.
    for table in ["/proc/net/tcp6", "/proc/net/tcp"] {
        if let Ok(text) = std::fs::read_to_string(table) {
            ports.extend(listening(&text));
        }
    }
    ports.retain(|port, _| DEV_PORTS.contains(port));
    ports
}

/// Asks the server at `address` for its front page. `None` when nothing answers HTTP there in
/// time; otherwise its title, when the page has one.
pub fn ask(address: SocketAddr, host: &str) -> Option<Option<String>> {
    let deadline = Instant::now() + PAGE_TIMEOUT;
    let mut stream = TcpStream::connect_timeout(&address, PAGE_TIMEOUT).ok()?;
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
    };
    stream.set_write_timeout(left()).ok()?;
    let request =
        format!("GET / HTTP/1.1\r\nHost: {host}\r\nAccept: text/html\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut page = Vec::new();
    let mut chunk = [0; 4096];
    while page.len() < PAGE_BYTES {
        let Some(time) = left() else { break };
        if stream.set_read_timeout(Some(time)).is_err() {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => page.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if !page.starts_with(b"HTTP/") {
        return None;
    }
    let text = String::from_utf8_lossy(&page);
    let body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    Some(title(body))
}

/// The page's `<title>`, decoded as `nameOf` in `localServers.ts` decodes it.
pub fn title(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title>")?;
    let raw = &page[start..end];
    if raw.contains('<') {
        return None;
    }
    let decoded = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let decoded = decoded.trim();
    (!decoded.is_empty()).then(|| decoded.chars().take(NAME_CHARS).collect())
}

/// What one scan keeps from the last: each answering port's name.
#[derive(Default)]
pub struct Scanner {
    names: BTreeMap<u16, String>,
}

impl Scanner {
    /// The servers among `listening`, in port order. `ask` is asked about ports not named yet;
    /// `excluded` (the agent bridge's port) is never listed.
    pub fn scan(
        &mut self,
        listening: &BTreeMap<u16, Family>,
        excluded: Option<u16>,
        mut ask: impl FnMut(SocketAddr, &str) -> Option<Option<String>>,
    ) -> Vec<LocalServer> {
        self.names
            .retain(|port, _| listening.contains_key(port) && Some(*port) != excluded);
        listening
            .iter()
            .filter(|(port, _)| Some(**port) != excluded)
            .filter_map(|(&port, family)| {
                let host = format!("localhost:{port}");
                let name = match self.names.entry(port) {
                    Entry::Occupied(name) => name.get().clone(),
                    Entry::Vacant(slot) => {
                        let answer = ask(SocketAddr::new(family.loopback(), port), &host)?;
                        slot.insert(answer.unwrap_or_else(|| host.clone())).clone()
                    }
                };
                Some(LocalServer {
                    port,
                    url: format!("http://{host}/"),
                    name,
                })
            })
            .collect()
    }
}

/// The background scan. Dropping it ends the thread once its current scan finishes, so quitting
/// never waits on a slow server.
pub struct LocalServerWatch {
    latest: Arc<Mutex<Option<Vec<LocalServer>>>>,
    excluded: Arc<AtomicU32>,
    _stop: mpsc::Sender<()>,
}

/// No excluded port in [`LocalServerWatch::excluded`]: outside the `u16` range.
const NONE: u32 = u32::MAX;

impl LocalServerWatch {
    /// Scans this machine's sockets and asks each new dev server for its page title.
    pub fn spawn() -> io::Result<Self> {
        let mut scanner = Scanner::default();
        Self::spawn_scan(move |skip| scanner.scan(&listening_dev_ports(), skip, ask))
    }

    /// Runs `scan` with the excluded port every [`POLL`].
    fn spawn_scan(
        mut scan: impl FnMut(Option<u16>) -> Vec<LocalServer> + Send + 'static,
    ) -> io::Result<Self> {
        let latest = Arc::new(Mutex::new(None));
        let excluded = Arc::new(AtomicU32::new(NONE));
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("flecha-local-servers".into())
            .spawn({
                let (latest, excluded) = (latest.clone(), excluded.clone());
                move || {
                    let mut shown = Vec::new();
                    loop {
                        let skip = u16::try_from(excluded.load(Ordering::Relaxed)).ok();
                        let found = scan(skip);
                        if found != shown {
                            shown.clone_from(&found);
                            *latest.lock().unwrap_or_else(|err| err.into_inner()) = Some(found);
                        }
                        match stopped.recv_timeout(POLL) {
                            Err(RecvTimeoutError::Timeout) => {}
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            })?;
        Ok(Self {
            latest,
            excluded,
            _stop: stop,
        })
    }

    /// The list, when it changed since the last call.
    pub fn take(&self) -> Option<Vec<LocalServer>> {
        self.latest
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    /// Leaves `port` out from the next scan on: Flecha's own agent bridge isn't a project.
    pub fn exclude(&self, port: Option<u16>) {
        self.excluded
            .store(port.map_or(NONE, u32::from), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    // Lines from a Linux host's tables, trimmed after the inode.
    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1
   1: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 2
   2: 0101A8C0:1389 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 3
   3: 0100007F:1435 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 4
   4: 0200007F:0FA0 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 6
   1: 00000000000000000000000000000000:2328 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 7
   2: 0000000000000000FFFF00000100007F:0FA1 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 8
   3: 000080FE00000000FF7A1D5C2E4F1A33:1770 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 9
";

    #[test]
    fn reads_loopback_and_wildcard_listeners_from_both_tables() {
        assert_eq!(
            listening(TCP),
            BTreeMap::from([(3000, Family::V4), (8080, Family::V4), (4000, Family::V4)]),
            "127.0.0.1, 0.0.0.0 and 127.0.0.2; not a LAN address or a connected socket"
        );
        assert_eq!(
            listening(TCP6),
            BTreeMap::from([(5173, Family::V6), (9000, Family::V6), (4001, Family::V4)]),
            "::1, :: and ::ffff:127.0.0.1; not a link-local address"
        );
    }

    #[test]
    fn finds_a_real_listener_in_the_kernels_table() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let table = std::fs::read_to_string("/proc/net/tcp").unwrap();
        assert_eq!(listening(&table).get(&port), Some(&Family::V4));
    }

    #[test]
    fn names_answering_servers_once_and_forgets_them_when_they_stop() {
        let mut scanner = Scanner::default();
        let asked = std::cell::RefCell::new(Vec::new());
        let ports = BTreeMap::from([(3000, Family::V4), (5173, Family::V6), (5432, Family::V4)]);
        let ask = |address: SocketAddr, host: &str| {
            asked.borrow_mut().push((address, host.to_owned()));
            match address.port() {
                3000 => Some(Some("My app".to_owned())),
                5173 => Some(None),
                _ => None,
            }
        };
        let servers = scanner.scan(&ports, None, &ask);
        assert_eq!(
            servers,
            [
                LocalServer {
                    port: 3000,
                    url: "http://localhost:3000/".into(),
                    name: "My app".into(),
                },
                LocalServer {
                    port: 5173,
                    url: "http://localhost:5173/".into(),
                    name: "localhost:5173".into(),
                },
            ]
        );
        assert_eq!(
            asked.borrow().iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [
                "127.0.0.1:3000".parse().unwrap(),
                "[::1]:5173".parse().unwrap(),
                "127.0.0.1:5432".parse().unwrap(),
            ]
        );
        assert_eq!(asked.borrow()[0].1, "localhost:3000");

        // Named ports aren't asked again; one that never answered is.
        asked.borrow_mut().clear();
        assert_eq!(scanner.scan(&ports, None, &ask).len(), 2);
        assert_eq!(asked.borrow().len(), 1);

        // The bridge's port is left out, and a stopped server's name forgotten.
        asked.borrow_mut().clear();
        let fewer = BTreeMap::from([(5173, Family::V6)]);
        assert!(scanner.scan(&fewer, Some(5173), &ask).is_empty());
        assert_eq!(scanner.scan(&ports, None, &ask).len(), 2);
        assert_eq!(
            asked.borrow().len(),
            3,
            "the forgotten servers are asked again"
        );
    }

    #[test]
    fn asks_a_server_for_its_title_over_http() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for body in ["<html><head><TITLE> Tom &amp; Jerry </TITLE>", "not http"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]).into_owned();
                let reply = if body == "not http" {
                    body.to_owned()
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n{body}")
                };
                stream.write_all(reply.as_bytes()).unwrap();
                assert!(request.starts_with("GET / HTTP/1.1\r\nHost: localhost:3000\r\n"));
            }
        });
        assert_eq!(
            ask(address, "localhost:3000"),
            Some(Some("Tom & Jerry".into()))
        );
        assert_eq!(ask(address, "localhost:3000"), None);
        server.join().unwrap();
    }

    #[test]
    fn titles_decode_entities_and_stop_at_sixty_characters() {
        assert_eq!(
            title("<title>&lt;b&gt; &quot;x&quot; &#39;y&#39;</title>").as_deref(),
            Some("<b> \"x\" 'y'")
        );
        assert_eq!(title("<title>   </title>"), None);
        assert_eq!(title("<title>unclosed"), None);
        let long = format!("<title>{}</title>", "é".repeat(80));
        assert_eq!(title(&long).unwrap().chars().count(), NAME_CHARS);
    }

    #[test]
    fn the_watch_reports_changes_and_stops_without_waiting_out_its_poll() {
        let (scanned, scans) = mpsc::channel();
        let watch = LocalServerWatch::spawn_scan(move |skip| {
            let _ = scanned.send(skip);
            vec![LocalServer {
                port: 5173,
                url: "http://localhost:5173/".into(),
                name: "Vite App".into(),
            }]
        })
        .unwrap();
        assert_eq!(scans.recv().unwrap(), None);
        // The first scan's list is taken once; an unchanged list isn't offered again.
        while watch.take().is_none() {
            std::thread::yield_now();
        }
        assert_eq!(watch.take(), None);

        watch.exclude(Some(3000));
        let latest = Arc::downgrade(&watch.latest);
        let started = Instant::now();
        drop(watch);
        // The thread holds the list until it ends.
        while latest.strong_count() > 0 {
            assert!(started.elapsed() < POLL, "the thread outlived its poll");
            std::thread::yield_now();
        }
    }
}
