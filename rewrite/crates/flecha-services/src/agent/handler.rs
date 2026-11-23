//! The MCP side: server identity, the tool list, and running a tool call against a backend.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use flecha_core::TabId;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
    Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::control::{
    BrowserControl, Evaluated, Navigation, ReadFormat, ToolError, ToolResult, TypeText,
};
use super::tools::{
    self, Args, DEFAULT_CONSOLE_LIMIT, DEFAULT_MAX_CHARS, DEFAULT_WAIT_MS, TOOL_TIMEOUT, WAIT_POLL,
};

/// The name and version `chromium/native-host/agentServer.ts` gives its `McpServer`.
const SERVER_NAME: &str = "flecha";
const SERVER_VERSION: &str = "0.1.0";

/// The newest revision with the `initialize` handshake, the one today's clients negotiate
/// with the TS server (`LATEST_PROTOCOL_VERSION` in `@modelcontextprotocol/sdk` 1.32.1).
pub const PROTOCOL_CEILING: ProtocolVersion = ProtocolVersion::LATEST_WITH_INITIALIZE;

/// The server's identity and capabilities, as the TS server reports them.
pub fn server_config() -> ServerConfig {
    ServerConfig::new(
        ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build(),
    )
    .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION))
    .with_protocol_version(PROTOCOL_CEILING)
}

/// `tools/list`, in `AGENT_TOOLS` order.
pub fn tool_list() -> Vec<Tool> {
    tools::TOOLS
        .iter()
        .map(|spec| Tool::new(spec.name, spec.description, Arc::new(spec.input_schema())))
        .collect()
}

/// One stateless request's handler; the HTTP service makes one per request.
pub(crate) struct AgentHandler<B> {
    backend: Arc<B>,
}

impl<B> AgentHandler<B> {
    pub(crate) fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B: BrowserControl> ServerHandler for AgentHandler<B> {
    fn get_info(&self) -> ServerConfig {
        server_config()
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&PROTOCOL_CEILING))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // A client that disconnects cancels its request; dropping the call stops the backend's work.
        tokio::select! {
            result = call(&*self.backend, &request.name, request.arguments.as_ref()) => Ok(result.into()),
            () = context.ct.cancelled() => Err(McpError::internal_error("request cancelled", None)),
        }
    }
}

/// Validates and runs one tool call within its deadline. Failures come back as tool errors
/// (`isError`), as the TS SDK reports them.
pub async fn call<B: BrowserControl>(
    backend: &B,
    name: &str,
    arguments: Option<&Map<String, Value>>,
) -> CallToolResult {
    let Some(spec) = tools::find(name) else {
        return failure(format!("MCP error -32602: Tool {name} not found"));
    };
    let args = match spec.validate(arguments) {
        Ok(args) => args,
        Err(message) => return failure(message),
    };
    // `toolTimeout` in chromium/shared/nativeMessaging.ts: one allowance plus the tool's own wait.
    let deadline = TOOL_TIMEOUT + Duration::from_millis(args.integer("timeoutMs").unwrap_or(0));
    match tokio::time::timeout(deadline, run(backend, spec.name, &args)).await {
        Ok(Ok(result)) => result,
        Ok(Err(ToolError(message))) => failure(message),
        Err(_) => failure(format!(
            "Flecha didn’t answer {name} within {}s",
            deadline.as_millis() as f64 / 1000.0
        )),
    }
}

async fn run<B: BrowserControl>(
    backend: &B,
    name: &str,
    args: &Args,
) -> ToolResult<CallToolResult> {
    let tab = || TabId::new(args.str("tabId").unwrap_or_default());
    Ok(match name {
        "list_tabs" => json_text(&backend.list_tabs().await?),
        "open_tab" => {
            let opened = backend
                .open_tab(args.str("url").unwrap_or_default())
                .await?;
            json_text(&json!({ "tabId": opened.id.as_str(), "url": opened.url }))
        }
        "activate_tab" => {
            backend.activate_tab(&tab()).await?;
            text("ok")
        }
        "close_tab" => {
            backend.close_tab(&tab()).await?;
            text("ok")
        }
        "navigate" => {
            // An empty `url` counts as none, as in the shell's `if (url)`.
            let to = match (
                args.str("url").filter(|url| !url.is_empty()),
                args.str("action"),
            ) {
                (Some(url), _) => Navigation::Url(url),
                (None, Some("back")) => Navigation::Back,
                (None, Some("forward")) => Navigation::Forward,
                (None, Some("reload")) => Navigation::Reload,
                (None, _) => Navigation::Settle,
            };
            let page = backend.navigate(&tab(), to).await?;
            json_text(&json!({ "url": page.url, "title": page.title }))
        }
        "read_page" => {
            let format = match args.str("format") {
                Some("html") => ReadFormat::Html,
                Some("links") => ReadFormat::Links,
                _ => ReadFormat::Text,
            };
            let max_chars = args.number("maxChars").unwrap_or(DEFAULT_MAX_CHARS);
            let page = backend.read_page(&tab(), format).await?;
            let end = js_index(max_chars, page.content.encode_utf16().count());
            text(&format!(
                "# {}\n{}\n\n{}",
                page.title,
                page.url,
                utf16_prefix(&page.content, end)
            ))
        }
        "screenshot" => plain(ContentBlock::image(
            backend.screenshot(&tab()).await?,
            "image/png",
        )),
        "console_logs" => {
            let level = args.str("level");
            let limit = args.number("limit").unwrap_or(DEFAULT_CONSOLE_LIMIT);
            let entries: Vec<_> = backend
                .console_logs(&tab())
                .await?
                .into_iter()
                .filter(|entry| level.is_none_or(|level| entry.level.as_str() == level))
                .collect();
            let start = js_index(-limit, entries.len());
            let lines: Vec<_> = entries[start..]
                .iter()
                .map(|entry| match &entry.source {
                    Some(source) if !source.is_empty() => {
                        format!("[{}] {}  ({source})", entry.level.as_str(), entry.message)
                    }
                    _ => format!("[{}] {}", entry.level.as_str(), entry.message),
                })
                .collect();
            let joined = lines.join("\n");
            text(if joined.is_empty() {
                "(no messages)"
            } else {
                &joined
            })
        }
        "click" => {
            let selector = args.str("selector").unwrap_or_default();
            match backend.click(&tab(), selector).await? {
                Some(url) => json_text(&json!({ "clicked": selector, "url": url })),
                None => failure(format!("No visible element matches {selector}")),
            }
        }
        "type" => {
            let input = TypeText {
                selector: args.str("selector"),
                text: args.str("text").unwrap_or_default(),
                clear: args.bool("clear"),
                submit: args.bool("submit"),
            };
            if backend.type_text(&tab(), input).await? {
                text("ok")
            } else {
                failure(format!(
                    "No element matches {}",
                    input.selector.unwrap_or_default()
                ))
            }
        }
        "press_key" => {
            backend
                .press_key(&tab(), args.str("key").unwrap_or_default())
                .await?;
            text("ok")
        }
        "evaluate" => match backend
            .evaluate(&tab(), args.str("expression").unwrap_or_default())
            .await?
        {
            Evaluated::Undefined => text("undefined"),
            Evaluated::Value(Value::String(value)) => text(&value),
            Evaluated::Value(value) => json_text(&value),
        },
        "wait_for" => wait_for(backend, &tab(), args).await?,
        other => failure(format!("MCP error -32602: Tool {other} not found")),
    })
}

/// Probes every `WAIT_POLL` until the page is ready or `timeoutMs` passes. The deadline and the
/// pauses are timers on the server's runtime, dropped when the call ends.
async fn wait_for<B: BrowserControl>(
    backend: &B,
    tab: &TabId,
    args: &Args,
) -> ToolResult<CallToolResult> {
    let timeout_ms = args.integer("timeoutMs").unwrap_or(DEFAULT_WAIT_MS);
    let timed_out = || failure(format!("Timed out after {timeout_ms}ms"));
    if timeout_ms == 0 {
        return Ok(timed_out());
    }
    let selector = args.str("selector");
    let ready = async {
        while !backend.probe(tab, selector).await? {
            tokio::time::sleep(WAIT_POLL).await;
        }
        Ok(text("ready"))
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), ready)
        .await
        .unwrap_or_else(|_| Ok(timed_out()))
}

/// A successful result as the TS SDK writes one: no `isError`, and no `resultType`, which the
/// handshake-based revisions do not have.
fn plain(content: ContentBlock) -> CallToolResult {
    let mut result = CallToolResult::success(vec![content]);
    result.is_error = None;
    result.result_type = None;
    result
}

fn text(value: &str) -> CallToolResult {
    plain(ContentBlock::text(value))
}

/// `JSON.stringify(value, null, 2)`, as `text()` in `agentTools.ts` writes non-string values.
fn json_text(value: &impl Serialize) -> CallToolResult {
    text(&serde_json::to_string_pretty(value).unwrap_or_default())
}

fn failure(message: String) -> CallToolResult {
    let mut result = CallToolResult::error(vec![ContentBlock::text(message)]);
    result.result_type = None;
    result
}

/// Where `Array.prototype.slice` puts the index `value` in a list of `len` items: truncated
/// toward zero, counted from the end when negative, clamped to the list.
fn js_index(value: f64, len: usize) -> usize {
    let value = if value.is_nan() { 0.0 } else { value.trunc() };
    let len = len as f64;
    (if value < 0.0 {
        (len + value).max(0.0)
    } else {
        value.min(len)
    }) as usize
}

/// The longest prefix of `text` within `units` UTF-16 code units. JavaScript would keep half of
/// a surrogate pair cut at the end; a Rust string cannot hold one, so the pair is left out.
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += ch.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}
