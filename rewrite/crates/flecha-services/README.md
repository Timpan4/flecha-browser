# flecha-services

Services that run beside the browser. So far this is the MCP agent bridge, ported from `chromium/native-host/agentServer.ts` and `chromium/shared/agentTools.ts`.

## Agent bridge

`AgentServer::start` serves MCP Streamable HTTP at `http://127.0.0.1:<port>/mcp`. It tries port 7390 and the nine ports after it, as the TS bridge does. The server is stateless, like the TS transport: no session id, and each answer arrives as an SSE stream.

The server needs a `BrowserControl` and nothing from CEF. A backend resolves tab ids in its flecha-core `BrowserState` and drives the engine. The helpers in `agent::state` place agent tabs, list tabs, and activate and close tabs by flecha-core's rules, so agent tabs land in the agent folder and Today's Clear leaves them alone. `agent::fake::FakeBrowser` holds a real `BrowserState` with in-memory pages, for tests and for `examples/fake_agent_server.rs`.

### Protocol

The server negotiates the handshake revisions rmcp 3.11.0 knows, up to 2025-11-25. That ceiling is `LATEST_PROTOCOL_VERSION` in `@modelcontextprotocol/sdk` 1.32.1, the SDK the TS bridge runs on. The current MCP revision is 2026-07-28, which drops the `initialize` handshake. It is not enabled, because today's clients negotiate 2025-11-25. The TS SDK also accepts the 2024-10-07 pre-release; rmcp does not, so such a client is offered 2025-11-25.

### Trust boundary

Requests are checked in this order before the MCP transport reads the body:

1. Exactly one `Host` header naming `127.0.0.1` or `localhost`, with an optional numeric port. An absolute request URI must name one of them too. Anything else gets 403.
2. An `Origin` header, when present, must be `http://127.0.0.1:<port>` or `http://localhost:<port>`. Anything else gets 403. The TS bridge did not check `Origin`, and the TS SDK client sends none.
3. Exactly one `Authorization` header equal to `Bearer <token>`, compared in constant time. Anything else gets 401.
4. The path must be exactly `/mcp`. Anything else gets 404.

The listener binds 127.0.0.1 only. rmcp then refuses bodies over 4 MiB with 413, the same bound as the TS SDK's `MAXIMUM_MESSAGE_SIZE`. It answers bodies it cannot read as JSON-RPC with 415.

Tool arguments are validated as zod validates them in the TS SDK. The checks are type, enum, integer and bounds. Unknown members are dropped, not refused, as zod's default object parse does. The error text matches the SDK's word for word.

### Token

`AgentToken` keeps the token in a file the application chooses, written with mode 0600 through a sibling file and a rename. A token file that is a symlink, belongs to another user, is readable or writable by others, or is malformed is replaced on open. Its token may have leaked. The Chromium shell kept the token in the profile's saved state. A separate file keeps it out of state exports, imports and set-aside copies. `renew` takes effect on the next request.

### Logs

The server thread drops rmcp's events below WARN. At DEBUG and TRACE rmcp logs whole JSON-RPC messages, which carry page text and addresses. The crate logs nothing of its own. `tests/agent_logging.rs` runs a session with every target at TRACE and checks that no token, page content or URL appears.

### Runtime

The server runs on its own thread, `flecha-mcp`, with a single-threaded Tokio runtime. Tool deadlines and the `wait_for` poll every 200 ms are timers on that runtime, dropped when the call ends. A client that disconnects cancels its call, and dropping the backend future stops its work. No timer reaches GPUI's executor. `tests/agent_load.rs` holds 4000 concurrent `wait_for` calls and checks that the thread stays idle once they finish.

### Differences from the TS bridge

- Only the exact path `/mcp` is served. The TS bridge accepted any path starting with `/mcp`.
- GET and DELETE on `/mcp` get 405. A stateless server may refuse them.
- Unreadable bodies get 415 from rmcp. The TS SDK answered 400.
- Tools carry no `execution: {"taskSupport": "forbidden"}`. rmcp's `Tool` has no such member, and clients read its absence as "forbidden".
- `read_page` with `maxChars` that splits a surrogate pair leaves the pair out. JavaScript kept half of it.

`tests/agent_tools_parity.rs` lists the schema and protocol differences and fails when one is added or disappears.

## Checks

```sh
# Regenerate the TS fixture after agentTools.ts changes; CI runs it with --check.
node rewrite/crates/flecha-services/scripts/agent-tools-fixture.mjs

# Drive the server with the TS SDK's client.
cargo build -p flecha-services --example fake_agent_server
node rewrite/crates/flecha-services/scripts/e2e-client.mjs rewrite/target/debug/examples/fake_agent_server

# Load measurements.
cargo test -p flecha-services --release --test agent_load -- --nocapture
```
