//! `cherrypi-fetch mcp`: a stdio MCP server with one tool, `fetch_page`.
//!
//! Servo starts on the first call and stays up for later ones. Renders run one
//! at a time. Logs go to stderr; stdout carries only JSON-RPC messages.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cherrypi_fetch::{RenderedPage, MAX_TIMEOUT, RENDER_TIMEOUT};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer, ServiceExt};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

const TOOL_NAME: &str = "fetch_page";
const DEFAULT_MAX_CHARS: usize = 40_000;
const MAX_CHARS_LIMIT: u64 = 200_000;
/// How long to wait for the MCP service to stop after a signal. rmcp cannot
/// interrupt a blocking read of stdin, so the service may keep waiting on it.
const STOP_GRACE: Duration = Duration::from_secs(2);

const TOOL_DESCRIPTION: &str = "Render a web page in Servo, a real browser engine, and return \
    its title and visible text. Use it when the page needs JavaScript: a single-page app, or a \
    page whose plain fetch comes back empty or says it is loading. An invalid argument or a \
    page that cannot be rendered is reported as a tool error. Private and loopback addresses are \
    blocked unless the server starts with mcp --allow-private.";

/// `structuredContent` of a successful `fetch_page` call. Its schema is the tool's `outputSchema`.
#[derive(Serialize, JsonSchema)]
struct FetchPageOutput {
    /// The URL as requested.
    url: String,
    /// The page's title, empty when it has none.
    title: String,
    /// Visible text, one line per block, cut to `max_chars`.
    text: String,
    /// Why the page did not settle before the timeout. Null when it settled.
    incomplete: Option<String>,
    /// Whether `text` was cut to `max_chars`.
    truncated: bool,
    /// Time spent rendering, in milliseconds. Time spent waiting for an earlier render is not counted.
    elapsed_ms: u64,
}

/// A validated `fetch_page` call.
#[derive(Debug, PartialEq)]
struct FetchRequest {
    raw_url: String,
    url: url::Url,
    timeout: Duration,
    max_chars: usize,
}

/// Serves `fetch_page`. Renders are serialized here: the render thread takes
/// one job at a time, and a call queued behind another would run out of its
/// reply time while it waited.
#[derive(Default)]
struct FetchServer {
    /// Held for the length of one render. `serve_stdio` also looks at it, to
    /// say when a shutdown has to wait for a render.
    render_lock: Arc<Mutex<()>>,
}

impl ServerHandler for FetchServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "cherrypi-fetch",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Renders JavaScript-heavy web pages in Servo and returns their visible text. \
                 Use fetch_page when a plain fetch of a page comes back empty.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![fetch_page_tool()]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() != TOOL_NAME {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let result = match parse_request(request.arguments.as_ref()) {
            Ok(fetch) => self.fetch(fetch, context).await,
            Err(message) => tool_error(message),
        };
        Ok(result.into())
    }
}

impl FetchServer {
    async fn fetch(
        &self,
        request: FetchRequest,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let cancelled = || tool_error("the call was cancelled before its render started");
        let _render = if let Ok(guard) = self.render_lock.try_lock() {
            guard
        } else {
            eprintln!("cherrypi-fetch: waiting for an earlier render to finish");
            // A cancelled call leaves the queue here, so it never starts a render.
            tokio::select! {
                guard = self.render_lock.lock() => guard,
                () = context.ct.cancelled() => return cancelled(),
            }
        };
        // Both branches of the select can be ready at once, and the lock can be
        // free before the client cancels. Check again before starting Servo's work.
        if context.ct.is_cancelled() {
            return cancelled();
        }
        // Once a render has started it cannot be stopped: Servo has no way to
        // abort a load. A cancel that arrives now runs the render to its end,
        // and its result is dropped.
        let started = Instant::now();
        let rendered = cherrypi_fetch::render_page(request.url, request.timeout).await;
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        drop(_render);
        match rendered {
            Ok(page) => success(request.raw_url, page, request.max_chars, elapsed_ms),
            Err(e) => tool_error(format!("render failed: {e}")),
        }
    }
}

fn fetch_page_tool() -> Tool {
    Tool::new(TOOL_NAME, TOOL_DESCRIPTION, input_schema()).with_output_schema::<FetchPageOutput>()
}

fn input_schema() -> JsonObject {
    match json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "http or https URL of the page to render."
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "maximum": 600,
                "default": 20,
                "description": "Seconds allowed for the page to load and settle, from 1 to 600. Default 20."
            },
            "max_chars": {
                "type": "integer",
                "minimum": 1,
                "default": 40000,
                "description": "Keep at most this many characters of text, from 1 to 200000. Larger values are clamped to 200000. Default 40000."
            }
        },
        "required": ["url"]
    }) {
        Value::Object(schema) => schema,
        _ => JsonObject::new(),
    }
}

/// Check the arguments of a `fetch_page` call. Every problem is a message for
/// the caller, never a protocol error. A null or missing optional field takes
/// its default.
fn parse_request(arguments: Option<&JsonObject>) -> Result<FetchRequest, String> {
    let empty = JsonObject::new();
    let arguments = arguments.unwrap_or(&empty);
    let raw_url = match arguments.get("url") {
        Some(Value::String(url)) => url.clone(),
        Some(_) => return Err("url must be a string".to_string()),
        None => return Err("url is required".to_string()),
    };
    let url = super::parse_url(&raw_url)?;

    let timeout_secs =
        optional_integer(arguments, "timeout_secs")?.unwrap_or_else(|| RENDER_TIMEOUT.as_secs());
    if !(1..=MAX_TIMEOUT.as_secs()).contains(&timeout_secs) {
        return Err(format!(
            "timeout_secs must be between 1 and {} (got {timeout_secs})",
            MAX_TIMEOUT.as_secs()
        ));
    }

    let max_chars = match optional_integer(arguments, "max_chars")? {
        None => DEFAULT_MAX_CHARS,
        Some(0) => return Err("max_chars must be at least 1".to_string()),
        Some(n) => usize::try_from(n.min(MAX_CHARS_LIMIT)).unwrap_or(usize::MAX),
    };

    Ok(FetchRequest {
        raw_url,
        url,
        timeout: Duration::from_secs(timeout_secs),
        max_chars,
    })
}

/// A whole, non-negative number. Some clients send whole numbers as floats
/// (`20.0`), so those are accepted too; `1.5` and strings are not.
fn optional_integer(arguments: &JsonObject, key: &str) -> Result<Option<u64>, String> {
    let whole = match arguments.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Number(number)) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|f| f.fract() == 0.0 && (0.0..u64::MAX as f64).contains(f))
                .map(|f| f as u64)
        }),
        Some(_) => None,
    };
    whole
        .map(Some)
        .ok_or_else(|| format!("{key} must be a whole number"))
}

/// Successful output: a text block the reader can scan, and the structured record.
fn success(
    raw_url: String,
    page: RenderedPage,
    max_chars: usize,
    elapsed_ms: u64,
) -> CallToolResult {
    let (text, truncated) = super::truncate(page.text, Some(max_chars));

    let mut body = format!("Title: {}\n", page.title);
    if let Some(reason) = &page.incomplete {
        body.push_str(&format!("Note: incomplete render: {reason}\n"));
    }
    if truncated {
        body.push_str(&format!(
            "Note: text truncated to the first {max_chars} characters (max_chars)\n"
        ));
    }
    body.push('\n');
    body.push_str(&text);

    let output = FetchPageOutput {
        url: raw_url,
        title: page.title,
        text,
        incomplete: page.incomplete,
        truncated,
        elapsed_ms,
    };
    let mut result = CallToolResult::success(vec![ContentBlock::text(body)]);
    result.structured_content = serde_json::to_value(&output).ok();
    result
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// Serve MCP over stdio until stdin closes or the process is interrupted, then
/// stop Servo and exit. Exit 0 on a clean stop, 1 if the server itself failed.
///
/// This ends the process itself rather than returning. The Tokio runtime waits
/// for its blocking threads when `main` returns, and the thread reading stdin
/// cannot be interrupted, so a signal with stdin still open would hang there.
pub async fn serve_stdio(allow_private: bool) -> ! {
    if let Err(e) = cherrypi_fetch::configure(cherrypi_fetch::RendererConfig {
        allow_private_networks: allow_private,
        ..Default::default()
    }) {
        eprintln!("cherrypi-fetch: MCP configuration failed: {e}");
        std::process::exit(1);
    }
    let server = FetchServer::default();
    let busy = server.render_lock.clone();
    let service = match server.serve(stdio()).await {
        Ok(service) => service,
        Err(e) => {
            eprintln!("cherrypi-fetch: MCP startup failed: {e}");
            std::process::exit(1);
        }
    };
    let cancel = service.cancellation_token();
    let mut quit = tokio::spawn(service.waiting());

    let outcome = tokio::select! {
        joined = &mut quit => flatten_join(joined),
        () = shutdown_signal() => {
            cancel.cancel();
            // Bounded: the service may stay blocked on stdin, which is fine. Go on after the grace period.
            match tokio::time::timeout(STOP_GRACE, &mut quit).await {
                Ok(joined) => flatten_join(joined),
                Err(_) => Ok(()),
            }
        }
    };

    if busy.try_lock().is_err() {
        eprintln!("cherrypi-fetch: stopping; waiting for the render in progress to finish");
    }
    // Servo may be partway through a render. This waits for that render to end,
    // so it runs on a blocking thread rather than a runtime worker.
    let _ = tokio::task::spawn_blocking(cherrypi_fetch::shutdown_renderer).await;

    let code = match outcome {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("cherrypi-fetch: MCP server failed: {message}");
            1
        }
    };
    std::process::exit(code)
}

/// The server task's result: a panic or a failed join is an error.
fn flatten_join(
    joined: Result<
        Result<rmcp::service::QuitReason, tokio::task::JoinError>,
        tokio::task::JoinError,
    >,
) -> Result<(), String> {
    match joined {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) | Err(e) => Err(e.to_string()),
    }
}

/// Resolves on SIGINT, or on SIGTERM where the platform has it.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> JsonObject {
        match value {
            Value::Object(map) => map,
            _ => panic!("test arguments must be an object"),
        }
    }

    fn parse(value: Value) -> Result<FetchRequest, String> {
        parse_request(Some(&args(value)))
    }

    #[test]
    fn url_is_required_and_must_be_http() {
        assert!(parse(json!({})).unwrap_err().contains("url is required"));
        assert!(parse(json!({"url": 5}))
            .unwrap_err()
            .contains("must be a string"));
        assert!(parse(json!({"url": "ftp://example.com/"}))
            .unwrap_err()
            .contains("scheme"));
        assert!(parse(json!({"url": "not a url"}))
            .unwrap_err()
            .contains("invalid URL"));
        assert!(parse(json!({"url": "https://example.com/"})).is_ok());
    }

    #[test]
    fn defaults_apply_when_optional_fields_are_missing_or_null() {
        let request = parse(json!({"url": "https://example.com/"})).unwrap();
        assert_eq!(request.timeout, Duration::from_secs(20));
        assert_eq!(request.max_chars, 40_000);

        let request = parse(json!({
            "url": "https://example.com/", "timeout_secs": null, "max_chars": null
        }))
        .unwrap();
        assert_eq!(request.timeout, Duration::from_secs(20));
        assert_eq!(request.max_chars, 40_000);
    }

    #[test]
    fn timeout_must_be_between_1_and_600_seconds() {
        for ok in [1, 600] {
            let request = parse(json!({"url": "https://a.test/", "timeout_secs": ok})).unwrap();
            assert_eq!(request.timeout, Duration::from_secs(ok));
        }
        // Whole numbers sent as floats are accepted.
        let request = parse(json!({"url": "https://a.test/", "timeout_secs": 20.0})).unwrap();
        assert_eq!(request.timeout, Duration::from_secs(20));
        let request = parse(json!({"url": "https://a.test/", "max_chars": 5.0})).unwrap();
        assert_eq!(request.max_chars, 5);
        for bad in [json!(0), json!(601), json!(-1), json!(1.5), json!("5")] {
            let err =
                parse(json!({"url": "https://a.test/", "timeout_secs": bad.clone()})).unwrap_err();
            assert!(!err.is_empty(), "{bad} was accepted");
        }
    }

    #[test]
    fn max_chars_is_clamped_to_the_limit_and_zero_is_refused() {
        let request = parse(json!({"url": "https://a.test/", "max_chars": 300_000})).unwrap();
        assert_eq!(request.max_chars, 200_000);
        let request = parse(json!({"url": "https://a.test/", "max_chars": 1})).unwrap();
        assert_eq!(request.max_chars, 1);
        assert!(parse(json!({"url": "https://a.test/", "max_chars": 0}))
            .unwrap_err()
            .contains("max_chars"));
        assert!(parse(json!({"url": "https://a.test/", "max_chars": "lots"})).is_err());
    }

    #[test]
    fn tool_declares_output_schema_for_the_structured_record() {
        let tool = fetch_page_tool();
        assert_eq!(tool.name, TOOL_NAME);
        let output = tool.output_schema.expect("outputSchema");
        let properties = output["properties"].as_object().unwrap();
        for key in [
            "url",
            "title",
            "text",
            "incomplete",
            "truncated",
            "elapsed_ms",
        ] {
            assert!(properties.contains_key(key), "missing {key}");
        }
    }

    #[test]
    fn input_schema_states_the_ranges_and_defaults() {
        let schema = input_schema();
        let properties = &schema["properties"];
        assert_eq!(properties["timeout_secs"]["minimum"], 1);
        assert_eq!(properties["timeout_secs"]["maximum"], 600);
        assert_eq!(properties["timeout_secs"]["default"], 20);
        assert_eq!(properties["max_chars"]["minimum"], 1);
        // No maximum for max_chars: larger values are clamped, not refused.
        assert!(properties["max_chars"].get("maximum").is_none());
        assert_eq!(properties["max_chars"]["default"], 40_000);
    }
}
