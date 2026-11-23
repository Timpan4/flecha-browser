//! The agent bridge over MCP Streamable HTTP, stateless, as `chromium/native-host/agentServer.ts`
//! serves it: 127.0.0.1 only, a local `Host` only, and the bearer token on every request.
//!
//! It runs on a thread of its own with a single-threaded Tokio runtime. Its sockets, deadlines
//! and `wait_for` pauses live on that runtime, never on the UI's executor.

use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::thread::JoinHandle;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN};
use http::{HeaderMap, HeaderName, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::control::BrowserControl;
use super::handler::AgentHandler;
use super::quiet;
use super::token::AgentToken;

/// The first port tried (`DEFAULT_AGENT_PORT` in `agentServer.ts`).
pub const DEFAULT_AGENT_PORT: u16 = 7390;
/// `agentServer.ts` tries the preferred port and the nine after it.
const PORT_ATTEMPTS: u16 = 10;
const MCP_PATH: &str = "/mcp";
/// The `Host` names `agentServer.ts` accepts, with any port.
const LOCAL_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];
const THREAD_NAME: &str = "flecha-mcp";
/// The pending-connection queue `agentServer.ts` had: the `backlog` default of Node's
/// `server.listen`. std's `TcpListener::bind` would use 128.
const LISTEN_BACKLOG: u32 = 511;

/// A running agent server. Dropping it stops the server and joins its thread.
pub struct AgentServer {
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AgentServer {
    /// Listens on 127.0.0.1, on `preferred_port` or one of the nine after it.
    pub fn start<B: BrowserControl>(
        backend: Arc<B>,
        token: Arc<AgentToken>,
        preferred_port: u16,
    ) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()?;
        let listener = {
            // Registers the socket with the runtime's driver; nothing runs on this thread.
            let _runtime = runtime.enter();
            (0..PORT_ATTEMPTS)
                .filter_map(|offset| preferred_port.checked_add(offset))
                .find_map(|port| {
                    bind_loopback(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into()).ok()
                })
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "No free port for the agent bridge",
                    )
                })?
        };
        let port = listener.local_addr()?.port();
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || {
                let _quiet = quiet::install();
                runtime.block_on(serve(listener, port, backend, token, stopped));
            })?;
        Ok(Self {
            port,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address agents connect to, as Settings shows it.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}{MCP_PATH}", self.port)
    }

    /// Stops accepting, ends open requests, and joins the server thread.
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Binds `addr` only if it is 127.0.0.1, the one address `agentServer.ts` listens on. Needs a
/// Tokio runtime context.
fn bind_loopback(addr: SocketAddr) -> io::Result<TcpListener> {
    if addr.ip() != Ipv4Addr::LOCALHOST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the agent bridge listens on 127.0.0.1 only",
        ));
    }
    let socket = TcpSocket::new_v4()?;
    // As Node's `server.listen` and std's `TcpListener::bind` set it.
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(LISTEN_BACKLOG)
}

async fn serve<B: BrowserControl>(
    listener: TcpListener,
    port: u16,
    backend: Arc<B>,
    token: Arc<AgentToken>,
    mut stopped: oneshot::Receiver<()>,
) {
    let shutdown = CancellationToken::new();
    let origins = LOCAL_HOSTS.map(|host| format!("http://{host}:{port}"));
    let config = StreamableHttpServerConfig::default()
        // Stateless, as the TS server runs its transport (no session id).
        .with_legacy_session_mode(false)
        // Answers come as SSE, as the TS transport sends them; no keep-alive or retry timers.
        .with_json_response(false)
        .with_sse_keep_alive(None)
        .with_sse_retry(None)
        .with_allowed_hosts(LOCAL_HOSTS)
        .with_allowed_origins(origins.clone())
        .with_cancellation_token(shutdown.clone());
    let mcp = StreamableHttpService::new(
        {
            let backend = backend.clone();
            move || Ok(AgentHandler::new(backend.clone()))
        },
        Arc::new(NeverSessionManager::default()),
        config,
    );
    let guard = Arc::new(Guard { token, origins });
    loop {
        let stream = tokio::select! {
            _ = &mut stopped => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(_) => continue,
            },
        };
        let guard = guard.clone();
        let mcp = mcp.clone();
        let service = service_fn(move |request: Request<Incoming>| {
            let guard = guard.clone();
            let mcp = mcp.clone();
            async move {
                Ok::<_, Infallible>(match guard.check(&request) {
                    Err((status, message)) => refuse(status, message),
                    Ok(()) => mcp.handle(request).await,
                })
            }
        });
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .serve_connection(TokioIo::new(stream), service);
            tokio::select! {
                _ = connection => {}
                () = shutdown.cancelled() => {}
            }
        });
    }
    shutdown.cancel();
}

/// The checks `agentServer.ts` makes before the MCP transport sees a request, plus the `Origin`
/// check the MCP spec requires against DNS rebinding.
struct Guard {
    token: Arc<AgentToken>,
    origins: [String; 2],
}

type Body = BoxBody<Bytes, Infallible>;

impl Guard {
    fn check<T>(&self, request: &Request<T>) -> Result<(), (StatusCode, &'static str)> {
        let headers = request.headers();
        let host_ok = single(headers, &HOST)
            .and_then(|host| host.to_str().ok())
            .is_some_and(local_host)
            && request
                .uri()
                .authority()
                .is_none_or(|authority| local_host(authority.as_str()));
        if !host_ok {
            return Err((StatusCode::FORBIDDEN, "Forbidden host"));
        }
        let origin_ok = match headers.get_all(ORIGIN).iter().count() {
            0 => true,
            1 => headers
                .get(ORIGIN)
                .and_then(|origin| origin.to_str().ok())
                .is_some_and(|origin| self.origins.iter().any(|allowed| allowed == origin)),
            _ => false,
        };
        if !origin_ok {
            return Err((StatusCode::FORBIDDEN, "Forbidden origin"));
        }
        let authorized = single(headers, &AUTHORIZATION)
            .is_some_and(|value| self.token.authorizes(value.as_bytes()));
        if !authorized {
            return Err((StatusCode::UNAUTHORIZED, "Missing or wrong token"));
        }
        if request.uri().path() != MCP_PATH {
            return Err((StatusCode::NOT_FOUND, "Not found"));
        }
        Ok(())
    }
}

/// The header's value when it appears exactly once.
fn single<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a http::HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

/// `127.0.0.1` or `localhost`, optionally with a numeric port, as `agentServer.ts` reads `Host`.
fn local_host(authority: &str) -> bool {
    let name = match authority.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => authority,
    };
    LOCAL_HOSTS.contains(&name)
}

fn refuse(status: StatusCode, message: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain")
        .body(Full::new(Bytes::from_static(message.as_bytes())).boxed())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv6Addr, SocketAddrV6};

    use super::*;

    #[test]
    fn binds_only_to_127_0_0_1() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let refused: [SocketAddr; 5] = [
            SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 2), 0).into(),
            SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 10), 0).into(),
            SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0).into(),
            SocketAddrV6::new(Ipv6Addr::LOCALHOST, 0, 0, 0).into(),
        ];
        for addr in refused {
            let err = bind_loopback(addr).expect_err("non-127.0.0.1 bind refused");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{addr}");
        }
        let listener = bind_loopback(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        assert_eq!(listener.local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn reads_host_as_the_ts_server_does() {
        for host in ["127.0.0.1", "127.0.0.1:7390", "localhost", "localhost:1"] {
            assert!(local_host(host), "{host}");
        }
        for host in [
            "",
            "localhost:",
            "localhost:x",
            "LOCALHOST",
            "localhost.evil.com",
            "127.0.0.1.evil.com",
            "evil.com:127.0.0.1",
            "127.0.0.1:80:80",
            "[::1]:7390",
            "0.0.0.0",
        ] {
            assert!(!local_host(host), "{host}");
        }
    }
}
