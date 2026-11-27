//! Runs `cherrypi-fetch mcp` over stdio: MCP handshake, `tools/list`,
//! `tools/call` against a local page, invalid input, and shutdown.
//!
//! Every line the server writes to stdout is parsed and checked to be a
//! JSON-RPC 2.0 message, so any stray output fails the test.
//!
//! A manual smoke check with the MCP Inspector, not run in CI:
//! `npx @modelcontextprotocol/inspector --cli target/debug/cherrypi-fetch mcp --method tools/list`
#![cfg(all(feature = "cli", feature = "mcp"))]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Renders can take a while the first time Servo starts.
const WAIT: Duration = Duration::from_secs(60);

/// The text is written by a script, so only a real render sees it.
const PAGE: &str = "<html><head><title>MCP Test</title></head><body>\
    <div id=\"app\">loading</div>\
    <script>setTimeout(function(){document.getElementById('app').textContent='mcp-rendered-marker';},200);</script>\
    </body></html>";

fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                PAGE.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    format!("http://{addr}/")
}

/// The server process plus its stdout, read on a thread so waits can time out.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<String>,
    next_id: u64,
}

impl Drop for Server {
    fn drop(&mut self) {
        // Only still running if a test panicked before finishing.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cherrypi-fetch"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            stdout: rx,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// Send a request and return its response, skipping anything else the
    /// server sends first. Every stdout line is checked as JSON-RPC.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.next_message();
            if message.get("id") == Some(&json!(id))
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                return message;
            }
        }
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn next_message(&mut self) -> Value {
        match self.stdout.recv_timeout(WAIT) {
            Ok(line) => parse_json_rpc(&line),
            Err(e) => panic!("no stdout line from the server ({e:?})"),
        }
    }

    /// Close stdin and wait for the server to exit. Returns its status after
    /// checking that stdout held nothing but JSON-RPC until it closed.
    fn finish(mut self) -> ExitStatus {
        drop(self.stdin.take());
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if started.elapsed() > WAIT {
                let _ = self.child.kill();
                panic!("server did not exit after stdin closed");
            }
            thread::sleep(Duration::from_millis(20));
        };
        loop {
            match self.stdout.recv_timeout(Duration::from_secs(5)) {
                Ok(line) => {
                    parse_json_rpc(&line);
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("stdout stayed open after exit"),
            }
        }
        status
    }
}

fn parse_json_rpc(line: &str) -> Value {
    let message: Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line:?}"));
    assert_eq!(
        message["jsonrpc"], "2.0",
        "stdout line is not JSON-RPC: {line}"
    );
    message
}

/// Run the handshake the MCP lifecycle requires before any other request.
fn start_session() -> Server {
    let mut server = Server::start();
    let init = server.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "mcp-test", "version": "0.0.0"}
        }),
    );
    assert!(init.get("error").is_none(), "initialize failed: {init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "cherrypi-fetch");
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "tools capability missing: {init}"
    );
    server.notify("notifications/initialized");
    server
}

fn call_fetch(server: &mut Server, arguments: Value) -> Value {
    let response = server.request(
        "tools/call",
        json!({"name": "fetch_page", "arguments": arguments}),
    );
    assert!(
        response.get("error").is_none(),
        "tools/call returned a protocol error: {response}"
    );
    response["result"].clone()
}

#[test]
fn serves_tools_list_and_fetches_a_page_over_stdio() {
    let url = serve();
    let mut server = start_session();

    let listed = server.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "{listed}");
    let tool = &tools[0];
    assert_eq!(tool["name"], "fetch_page");
    assert!(
        tool["inputSchema"]["properties"]["url"].is_object(),
        "{tool}"
    );
    let output_schema = &tool["outputSchema"];
    for key in [
        "url",
        "title",
        "text",
        "incomplete",
        "truncated",
        "elapsed_ms",
    ] {
        assert!(
            output_schema["properties"][key].is_object(),
            "outputSchema lacks {key}: {output_schema}"
        );
    }

    let result = call_fetch(&mut server, json!({"url": url}));
    assert!(
        result.get("isError").is_none() || result["isError"] == false,
        "{result}"
    );
    let content = result["content"][0]["text"].as_str().expect("text content");
    assert!(content.starts_with("Title: MCP Test"), "{content}");
    assert!(content.contains("mcp-rendered-marker"), "{content}");

    let structured = &result["structuredContent"];
    assert_eq!(structured["url"], url.as_str());
    assert_eq!(structured["title"], "MCP Test");
    assert!(structured["text"]
        .as_str()
        .unwrap()
        .contains("mcp-rendered-marker"));
    assert!(structured["incomplete"].is_null(), "{structured}");
    assert_eq!(structured["truncated"], false);
    assert!(structured["elapsed_ms"].is_u64(), "{structured}");

    // Renders reuse the same Servo instance: a second call also works.
    let again = call_fetch(&mut server, json!({"url": url, "max_chars": 10}));
    assert_eq!(again["structuredContent"]["truncated"], true, "{again}");
    assert_eq!(
        again["structuredContent"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        10
    );

    let status = server.finish();
    assert_eq!(status.code(), Some(0), "closing stdin exits 0");
}

#[test]
fn invalid_input_is_a_tool_error_not_a_protocol_error() {
    let mut server = start_session();
    let cases = [
        json!({}),
        json!({"url": "ftp://example.com/"}),
        json!({"url": "not a url"}),
        json!({"url": 5}),
        json!({"url": "http://127.0.0.1:9/", "timeout_secs": 0}),
        json!({"url": "http://127.0.0.1:9/", "timeout_secs": 601}),
        json!({"url": "http://127.0.0.1:9/", "timeout_secs": "5"}),
        json!({"url": "http://127.0.0.1:9/", "max_chars": 0}),
        json!({"url": "http://127.0.0.1:9/", "max_chars": "lots"}),
    ];
    for arguments in cases {
        let result = call_fetch(&mut server, arguments.clone());
        assert_eq!(result["isError"], true, "{arguments} gave {result}");
        assert!(
            result.get("structuredContent").is_none(),
            "{arguments} gave structured content: {result}"
        );
        assert!(
            result["content"][0]["text"]
                .as_str()
                .is_some_and(|t| !t.is_empty()),
            "{arguments} gave no message: {result}"
        );
    }
    let status = server.finish();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn unknown_tools_are_protocol_errors() {
    let mut server = start_session();
    let response = server.request("tools/call", json!({"name": "no_such_tool"}));
    assert!(response.get("error").is_some(), "{response}");
    let status = server.finish();
    assert_eq!(status.code(), Some(0));
}

#[cfg(unix)]
#[test]
fn sigterm_shuts_the_server_down_cleanly() {
    let url = serve();
    let mut server = start_session();
    let _ = call_fetch(&mut server, json!({"url": url}));

    let pid = server.child.id();
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());

    let status = server.finish();
    assert_eq!(status.code(), Some(0), "SIGTERM exits 0");
}
