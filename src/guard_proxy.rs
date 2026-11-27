//! The checking proxy. Servo sends every http and https connection it makes
//! through this proxy as `CONNECT host:port`, so the address check runs at
//! connect time, on the address the connection actually uses. Servo never
//! resolves a host itself, so DNS rebinding cannot move a connection to a
//! private address after the check.
//!
//! The proxy listens on `127.0.0.1` on a random port, and requires a random
//! per-process token in `Proxy-Authorization`, so other local processes cannot
//! use it. Servo reads its address from the preferences `network_http_proxy_uri`
//! and `network_https_proxy_uri`; see `render_thread`.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

use crate::check_host;

/// The user name in the proxy URL. Servo sends it with the token as Basic auth.
const USERNAME: &str = "cherrypi";
/// Largest request head the proxy reads before it gives up on a client.
const MAX_HEAD: usize = 8 * 1024;
/// Bytes copied per read while relaying a tunnel.
const CHUNK: usize = 16 * 1024;

/// How the proxy behaves. `allow_private` only skips the address check; the
/// port and self-address refusals always apply.
#[derive(Clone, Debug)]
pub(crate) struct Settings {
    pub allow_private: bool,
    /// Time allowed for the client to send its request head.
    pub head_timeout: Duration,
    /// Time allowed for resolving and checking a destination.
    pub check_timeout: Duration,
    /// Time allowed for each attempt to connect to a checked address.
    pub connect_timeout: Duration,
    /// A tunnel with no bytes in either direction for this long is closed.
    pub idle_timeout: Duration,
}

/// A running proxy. Dropping the runtime that runs it stops it.
pub(crate) struct GuardProxy {
    addr: SocketAddr,
    token: String,
}

impl GuardProxy {
    /// The proxy URL for Servo's preferences. The password is the token.
    pub(crate) fn url(&self) -> String {
        format!("http://{USERNAME}:{}@{}", self.token, self.addr)
    }

    #[cfg(test)]
    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }
}

/// Tunnels the proxy has opened since the process started.
static TUNNELS: AtomicU64 = AtomicU64::new(0);

/// How many tunnels the proxy has opened, so a test can show that a request
/// went through the check.
pub(crate) fn tunnels_opened() -> u64 {
    TUNNELS.load(Ordering::Relaxed)
}

/// Shared by every connection the proxy accepts.
struct State {
    addr: SocketAddr,
    expected_auth: String,
    settings: Settings,
}

/// Bind the proxy on a random loopback port and start accepting. Must run inside
/// a Tokio runtime, which keeps the proxy alive for as long as it runs.
pub(crate) async fn start(settings: Settings) -> io::Result<GuardProxy> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let token = new_token()?;
    let state = Arc::new(State {
        addr,
        expected_auth: basic_auth(&token),
        settings,
    });
    tokio::spawn(accept_loop(listener, state));
    Ok(GuardProxy { addr, token })
}

/// 32 random bytes as hex, so the token needs no escaping in a URL.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn basic_auth(token: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{USERNAME}:{token}"))
    )
}

async fn accept_loop(listener: TcpListener, state: Arc<State>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = state.clone();
                tokio::spawn(async move { handle(stream, &state).await });
            }
            Err(e) => {
                tracing::warn!(error = %e, "the guard proxy could not accept a connection");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle(mut stream: TcpStream, state: &State) {
    let (head, leftover) =
        match tokio::time::timeout(state.settings.head_timeout, read_head(&mut stream)).await {
            Ok(Ok(parts)) => parts,
            _ => return reply(&mut stream, "400 Bad Request", "").await,
        };
    let Ok(text) = std::str::from_utf8(&head) else {
        return reply(&mut stream, "400 Bad Request", "").await;
    };
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    // Authentication comes first, so an unauthenticated client learns nothing
    // about what the proxy would accept.
    let authorized = lines.take_while(|line| !line.is_empty()).any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("proxy-authorization")
                && value.trim() == state.expected_auth
        })
    });
    if !authorized {
        return reply(
            &mut stream,
            "407 Proxy Authentication Required",
            "Proxy-Authenticate: Basic realm=\"cherrypi-fetch\"\r\n",
        )
        .await;
    }
    if method != "CONNECT" {
        return reply(&mut stream, "405 Method Not Allowed", "Allow: CONNECT\r\n").await;
    }
    let Some((host, port)) = parse_target(target) else {
        return reply(&mut stream, "400 Bad Request", "").await;
    };

    let allow = state.settings.allow_private;
    let addrs = match tokio::time::timeout(
        state.settings.check_timeout,
        check_host(&host, port, allow),
    )
    .await
    {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(e)) => {
            tracing::debug!(host = %host, port, error = %e, "the guard proxy refused a connection");
            return reply(&mut stream, "403 Forbidden", "").await;
        }
        Err(_) => {
            tracing::debug!(host = %host, port, "the guard proxy timed out checking a host");
            return reply(&mut stream, "504 Gateway Timeout", "").await;
        }
    };
    // The proxy's own listener is never a destination, even when private
    // networks are allowed: a tunnel to it would reach this proxy again.
    if addrs
        .iter()
        .any(|ip| SocketAddr::new(*ip, port) == state.addr)
    {
        tracing::debug!(host = %host, port, "the guard proxy refused its own address");
        return reply(&mut stream, "403 Forbidden", "").await;
    }

    let upstream = match connect_checked(&addrs, port, state.settings.connect_timeout).await {
        Ok(upstream) => upstream,
        Err(e) => {
            tracing::debug!(host = %host, port, error = %e, "the guard proxy could not connect");
            return reply(&mut stream, "502 Bad Gateway", "").await;
        }
    };
    if stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    TUNNELS.fetch_add(1, Ordering::Relaxed);
    let mut upstream = upstream;
    if !leftover.is_empty() && upstream.write_all(&leftover).await.is_err() {
        return;
    }
    relay(stream, upstream, state.settings.idle_timeout).await;
}

/// Read up to the end of a request head. Returns the head and any bytes read
/// after it, which belong to the tunnel.
async fn read_head(stream: &mut TcpStream) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&chunk[..n]);
        if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            let leftover = head.split_off(end + 4);
            head.truncate(end);
            return Ok((head, leftover));
        }
        if head.len() > MAX_HEAD {
            return Err(io::Error::other("request head too large"));
        }
    }
}

/// Split a CONNECT target into a normalised host and a port. The host is
/// normalised the way a URL is, so `0x7f.1` becomes `127.0.0.1` before the
/// check sees it. A target without an explicit port, or with user info or a
/// path, is malformed.
fn parse_target(target: &str) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = target.strip_prefix('[') {
        let (inner, after) = rest.split_once(']')?;
        (format!("[{inner}]"), after.strip_prefix(':')?)
    } else {
        let (host, port) = target.rsplit_once(':')?;
        (host.to_string(), port)
    };
    if host.is_empty() || host.contains(['@', '/', '?', '#', '\\', ' ']) {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    let parsed = url::Url::parse(&format!("https://{host}/")).ok()?;
    Some((parsed.host_str()?.to_string(), port))
}

/// Connect to the first checked address that answers. Only addresses the
/// check returned are tried, so a second DNS answer can never be used.
async fn connect_checked(addrs: &[IpAddr], port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::other("no address to connect to");
    for ip in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(SocketAddr::new(*ip, port))).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => last = e,
            Err(_) => last = io::ErrorKind::TimedOut.into(),
        }
    }
    Err(last)
}

/// Copy both ways until both sides are done, or until no bytes have moved for
/// `idle`. A slow but live tunnel is kept; an abandoned one is closed.
async fn relay(client: TcpStream, upstream: TcpStream, idle: Duration) {
    let (client_read, client_write) = client.into_split();
    let (upstream_read, upstream_write) = upstream.into_split();
    let moved = Arc::new(AtomicU64::new(0));
    let up = pipe(client_read, upstream_write, moved.clone());
    let down = pipe(upstream_read, client_write, moved.clone());
    tokio::select! {
        _ = async { tokio::join!(up, down); } => {}
        _ = idle_watch(&moved, idle) => {}
    }
}

async fn pipe(mut read: OwnedReadHalf, mut write: OwnedWriteHalf, moved: Arc<AtomicU64>) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        match read.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                moved.fetch_add(n as u64, Ordering::Relaxed);
                if write.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = write.shutdown().await;
}

/// Returns once a whole `idle` period passes with no bytes moved.
async fn idle_watch(moved: &AtomicU64, idle: Duration) {
    let mut seen = moved.load(Ordering::Relaxed);
    loop {
        tokio::time::sleep(idle).await;
        let now = moved.load(Ordering::Relaxed);
        if now == seen {
            return;
        }
        seen = now;
    }
}

async fn reply(stream: &mut TcpStream, status: &str, extra_headers: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set_test_dns;
    use std::sync::atomic::AtomicUsize;

    const PUBLIC: &str = "93.184.216.34";

    fn settings(allow_private: bool) -> Settings {
        Settings {
            allow_private,
            head_timeout: Duration::from_secs(2),
            check_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(1),
            idle_timeout: Duration::from_secs(60),
        }
    }

    /// The proxy's token, read back from its URL.
    fn token_of(proxy: &GuardProxy) -> String {
        let url = url::Url::parse(&proxy.url()).unwrap();
        url.password().unwrap().to_string()
    }

    fn connect_request(target: &str, auth: &str) -> String {
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: {auth}\r\n\r\n"
        )
    }

    /// Send `request` and return the response's status line.
    async fn status_of(proxy: &GuardProxy, request: &str) -> String {
        let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let (head, _) = tokio::time::timeout(Duration::from_secs(10), read_head(&mut stream))
            .await
            .expect("the proxy answers")
            .expect("a response head");
        String::from_utf8(head)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string()
    }

    /// A loopback server that counts the connections it accepts and echoes.
    async fn echo_server() -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 256];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (port, hits)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_a_connect_without_the_token_with_407() {
        let proxy = start(settings(false)).await.unwrap();
        let none = connect_request("93.184.216.34:443", "");
        assert!(status_of(&proxy, &none).await.starts_with("HTTP/1.1 407"));
        let wrong = connect_request("93.184.216.34:443", &basic_auth("not-the-token"));
        assert!(status_of(&proxy, &wrong).await.starts_with("HTTP/1.1 407"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn answers_other_methods_with_405_when_authorized() {
        let proxy = start(settings(false)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let get =
            format!("GET http://93.184.216.34/ HTTP/1.1\r\nProxy-Authorization: {auth}\r\n\r\n");
        assert!(status_of(&proxy, &get).await.starts_with("HTTP/1.1 405"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_a_loopback_target_and_never_connects_to_it() {
        let (port, hits) = echo_server().await;
        let proxy = start(settings(false)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let target = format!("127.0.0.1:{port}");
        assert!(status_of(&proxy, &connect_request(&target, &auth))
            .await
            .starts_with("HTTP/1.1 403"));
        assert_eq!(hits.load(Ordering::SeqCst), 0, "loopback was connected to");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_bad_ports_before_resolving_the_host() {
        set_test_dns("bad-port.test", vec![vec![PUBLIC.parse().unwrap()]]);
        let proxy = start(settings(false)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        for port in ["0", "25", "6000", "6667"] {
            let target = format!("bad-port.test:{port}");
            assert!(
                status_of(&proxy, &connect_request(&target, &auth))
                    .await
                    .starts_with("HTTP/1.1 403"),
                "port {port} was not refused"
            );
        }
        assert_eq!(
            crate::test_lookup_count("bad-port.test"),
            0,
            "a bad port was resolved before it was refused"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_its_own_address_even_when_private_networks_are_allowed() {
        let proxy = start(settings(true)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let target = format!("127.0.0.1:{}", proxy.addr().port());
        assert!(status_of(&proxy, &connect_request(&target, &auth))
            .await
            .starts_with("HTTP/1.1 403"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resolves_once_and_connects_only_to_the_checked_address() {
        // The first answer is public and passes the check; a second answer
        // would be loopback, where the echo server listens. The proxy must
        // not ask DNS again, and must not reach loopback.
        let (port, hits) = echo_server().await;
        set_test_dns(
            "rebind-guard.test",
            vec![
                vec![PUBLIC.parse().unwrap()],
                vec!["127.0.0.1".parse().unwrap()],
            ],
        );
        let proxy = start(settings(false)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let _ = status_of(
            &proxy,
            &connect_request(&format!("rebind-guard.test:{port}"), &auth),
        )
        .await;
        assert_eq!(crate::test_lookup_count("rebind-guard.test"), 1);
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the proxy connected to loopback after a rebind"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn relays_bytes_to_an_allowed_target() {
        let (port, _) = echo_server().await;
        let proxy = start(settings(true)).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
        stream
            .write_all(connect_request(&format!("127.0.0.1:{port}"), &auth).as_bytes())
            .await
            .unwrap();
        let (head, leftover) = read_head(&mut stream).await.unwrap();
        assert!(String::from_utf8(head).unwrap().starts_with("HTTP/1.1 200"));
        assert!(leftover.is_empty());
        stream.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut echoed))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(&echoed, b"ping");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closes_a_tunnel_that_goes_idle() {
        let (port, _) = echo_server().await;
        let mut quick = settings(true);
        quick.idle_timeout = Duration::from_millis(300);
        let proxy = start(quick).await.unwrap();
        let auth = basic_auth(&token_of(&proxy));
        let mut stream = TcpStream::connect(proxy.addr()).await.unwrap();
        stream
            .write_all(connect_request(&format!("127.0.0.1:{port}"), &auth).as_bytes())
            .await
            .unwrap();
        let _ = read_head(&mut stream).await.unwrap();
        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
            .await
            .expect("the idle tunnel is closed");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "the idle tunnel stayed open"
        );
    }

    #[test]
    fn parses_targets_and_normalises_hosts() {
        assert_eq!(
            parse_target("example.com:443"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(parse_target("[::1]:8080"), Some(("[::1]".into(), 8080)));
        assert_eq!(parse_target("0x7f.1:80"), Some(("127.0.0.1".into(), 80)));
        assert_eq!(parse_target("example.com"), None, "no port");
        assert_eq!(parse_target("example.com:99999"), None, "port out of range");
        assert_eq!(parse_target("user@example.com:80"), None, "user info");
        assert_eq!(parse_target("example.com/x:80"), None, "a path");
        assert_eq!(parse_target(":80"), None, "no host");
    }

    #[test]
    fn basic_auth_is_user_colon_token_in_base64() {
        assert_eq!(basic_auth("t0ken"), "Basic Y2hlcnJ5cGk6dDBrZW4=");
    }

    #[test]
    fn tokens_are_fresh_and_url_safe() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
