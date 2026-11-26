//! Loopback HTTP servers and render helpers for the redirect tests. Each
//! server records every request it answers, so a test can check what a
//! redirect hop actually sent.
//!
//! Every redirect test is its own test binary: the renderer runs Servo once
//! per process and cannot start again after `shutdown_renderer`.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cherrypi_fetch::{
    configure, render_page, set_test_public_origins, RenderedPage, RendererConfig,
};

/// One request a server received.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    /// The request target, such as `/a?b=c`.
    pub path: String,
    /// Header names and values, names as sent.
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    /// The first value of `name`, compared without regard to case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The path without its query string.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }
}

/// What a server answers with.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    /// A 200 with the given body and `text/html`.
    pub fn html(body: impl Into<String>) -> Self {
        Self::ok_with_type(body.into().into_bytes(), "text/html; charset=utf-8")
    }

    /// A 200 with the given body and `text/plain`.
    pub fn text(body: impl Into<String>) -> Self {
        Self::ok_with_type(body.into().into_bytes(), "text/plain; charset=utf-8")
    }

    /// A 200 with raw bytes and the given content type.
    pub fn ok_with_type(body: Vec<u8>, content_type: &str) -> Self {
        Self {
            status: "200 OK",
            headers: vec![("Content-Type".into(), content_type.into())],
            body,
        }
    }

    /// A 302 to `location`, with no body.
    pub fn redirect(location: &str) -> Self {
        Self {
            status: "302 Found",
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
        }
    }

    pub fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            headers: Vec::new(),
            body: b"not found".to_vec(),
        }
    }

    /// Add a header, such as `Set-Cookie` or `Content-Encoding`.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// A running loopback server.
pub struct Server {
    pub port: u16,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Server {
    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// The requests whose path (without query) is `route`.
    pub fn hits(&self, route: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.route() == route)
            .collect()
    }
}

/// Serve on `addr` (for example `"127.0.0.1:0"`). Each request is answered
/// with `route(&request)` and then the connection is closed.
pub fn serve<F>(addr: &str, route: F) -> Server
where
    F: Fn(&Recorded) -> Reply + Send + Sync + 'static,
{
    let listener = TcpListener::bind(addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let route = Arc::new(route);
    let log = requests.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let route = route.clone();
            let log = log.clone();
            thread::spawn(move || handle(stream, route.as_ref(), &log));
        }
    });
    Server { port, requests }
}

fn handle<F>(mut stream: TcpStream, route: &F, log: &Mutex<Vec<Recorded>>)
where
    F: Fn(&Recorded) -> Reply,
{
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let recorded = Recorded {
        method: method.clone(),
        path,
        headers,
    };
    let reply = route(&recorded);
    if let Ok(mut log) = log.lock() {
        log.push(recorded);
    }
    let mut head = format!("HTTP/1.1 {}\r\n", reply.status);
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    let _ = stream.write_all(head.as_bytes());
    // A HEAD response carries the headers of the GET it stands for, but no body.
    if method != "HEAD" {
        let _ = stream.write_all(&reply.body);
    }
}

/// Configure the renderer for these tests and clear any proxy variables, which
/// would make the render refuse to start.
pub fn setup() {
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        std::env::remove_var(name);
    }
    configure(RendererConfig {
        user_agent: "redirect-test/1.0".into(),
        allow_private_networks: false,
    })
    .expect("configure the renderer");
}

/// Mark these host and port pairs as public for the address check.
pub fn mark_public(origins: &[(&str, u16)]) {
    set_test_public_origins(origins);
}

/// Render `url` with a generous budget.
pub async fn render(url: &str) -> Result<RenderedPage, cherrypi_fetch::RenderError> {
    render_page(url::Url::parse(url).unwrap(), Duration::from_secs(10)).await
}
