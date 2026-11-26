//! Renders JavaScript-heavy pages in Servo and returns the visible text.
//!
//! ```no_run
//! # async fn demo() -> Result<(), cherrypi_fetch::RenderError> {
//! cherrypi_fetch::configure(cherrypi_fetch::RendererConfig::default())?; // optional
//! let url = url::Url::parse("https://example.com/").unwrap();
//! let result = cherrypi_fetch::render_page(url, cherrypi_fetch::RENDER_TIMEOUT).await;
//! cherrypi_fetch::shutdown_renderer(); // before the process exits, on every path
//! let page = result?;
//! println!("{}\n{}", page.title, page.text);
//! # Ok(()) }
//! ```
//!
//! Servo can run only once per process, and its `WebView` is `!Send` (it
//! holds `Rc`), so one OS thread owns the `Servo` instance and renders pages
//! one at a time. It starts on the first [`render_page`] call, which sends a
//! job over a channel and waits with a deadline. Settings are therefore
//! process-wide: [`configure`] them before the first render. Every render
//! starts with no cookies, storage or cache from earlier renders.
//!
//! Two things keep a misbehaving page from hanging a render:
//!
//! - The page's own same-origin GET requests from scripts (XHR and fetch) are
//!   made by this module with [`REQUEST_TIMEOUT`], using the page's cookies,
//!   and handed back to Servo. Servo has no response timeout, so a synchronous
//!   XHR to a server that never answers would otherwise block the page's
//!   script thread for good.
//! - If the page's scripts stop answering anyway, the render returns the last
//!   text it captured and says that the page is incomplete.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::rc::Rc;
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use content_security_policy::Destination;
use dpi::PhysicalSize;
use reqwest::header::{
    HeaderMap, HeaderValue, COOKIE, REFERER, SET_COOKIE, USER_AGENT as UA_HEADER,
};
use servo::{
    CookieSource, EventLoopWaker, JSValue, LoadStatus, Preferences, RenderingContext, Servo,
    ServoBuilder, ServoDelegate, SoftwareRenderingContext, StorageType, WebResourceLoad,
    WebResourceResponse, WebView, WebViewBuilder, WebViewDelegate,
};

/// User-Agent sent when [`configure`] is not called. It claims Windows on
/// purpose: some sites hang requests from desktop Linux user agents.
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:160.0) Gecko/20100101 Firefox/160.0";

/// Process-wide renderer settings. See [`configure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererConfig {
    /// User-Agent for the page and every request it makes.
    pub user_agent: String,
    /// Allow the page and its resources to reach loopback, private and other
    /// non-public addresses. Off by default; turn it on only for local
    /// testing.
    pub allow_private_networks: bool,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            user_agent: DEFAULT_USER_AGENT.to_string(),
            allow_private_networks: false,
        }
    }
}

/// Why a render, or the renderer's configuration, failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The User-Agent cannot be sent as an HTTP header value.
    #[error("invalid user agent: it must be non-empty and valid in an HTTP header")]
    InvalidUserAgent,
    /// The renderer already has different settings. They are fixed once set,
    /// or once the first render starts.
    #[error("the renderer is already configured with different settings")]
    AlreadyConfigured,
    /// The timeout is zero or longer than [`MAX_TIMEOUT`].
    #[error("invalid timeout: it must be between 1 second and {} seconds", MAX_TIMEOUT.as_secs())]
    InvalidTimeout,
    /// The page could not be rendered: startup failed, the page produced no
    /// text in time, or the render thread stopped answering.
    #[error("{0}")]
    Failed(String),
}

static CONFIG: OnceLock<RendererConfig> = OnceLock::new();

/// Set the renderer's settings for this process. Call it before the first
/// render; without it, [`RendererConfig::default`] is used. Calling it again
/// with the same settings is fine. Different settings, or any settings after a
/// render has already used the defaults, give [`RenderError::AlreadyConfigured`].
pub fn configure(config: RendererConfig) -> Result<(), RenderError> {
    if config.user_agent.trim().is_empty() || HeaderValue::from_str(&config.user_agent).is_err() {
        return Err(RenderError::InvalidUserAgent);
    }
    let current = CONFIG.get_or_init(|| config.clone());
    if *current == config {
        Ok(())
    } else {
        Err(RenderError::AlreadyConfigured)
    }
}

/// The settings in effect, fixing the defaults if none were configured.
fn config() -> &'static RendererConfig {
    CONFIG.get_or_init(RendererConfig::default)
}

fn user_agent() -> &'static str {
    &config().user_agent
}

fn allow_private_networks() -> bool {
    config().allow_private_networks
}

/// Whether `ip` is a public destination. Loopback, private, link-local,
/// unspecified, broadcast, shared (CGNAT), documentation, benchmarking and
/// multicast addresses are not public, and an IPv4-mapped IPv6 address is
/// checked as its IPv4 address.
fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_ipv4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_ipv4(v4),
            None => is_public_ipv6(v6),
        },
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(o[0] == 0 // 0.0.0.0/8 "this network"
        || o[0] == 10 // 10.0.0.0/8
        || o[0] == 127 // 127.0.0.0/8 loopback
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        || (o[0] == 169 && o[1] == 254) // 169.254.0.0/16 link-local
        || (o[0] == 172 && (o[1] & 0xf0) == 16) // 172.16.0.0/12
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
        || (o[0] == 192 && o[1] == 0 && o[2] == 2) // 192.0.2.0/24 TEST-NET-1
        || (o[0] == 192 && o[1] == 88 && o[2] == 99) // 192.88.99.0/24
        || (o[0] == 192 && o[1] == 168) // 192.168.0.0/16
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18.0.0/15
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // 198.51.100.0/24
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // 203.0.113.0/24
        || o[0] >= 224) // 224.0.0.0/4 multicast and 240.0.0.0/4 reserved
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    let s = ip.segments();
    // Unique local fc00::/7.
    if (s[0] & 0xfe00) == 0xfc00 {
        return false;
    }
    // Link-local fe80::/10.
    if (s[0] & 0xffc0) == 0xfe80 {
        return false;
    }
    // Deprecated site-local fec0::/10.
    if (s[0] & 0xffc0) == 0xfec0 {
        return false;
    }
    // Teredo 2001::/32, documentation 2001:db8::/32, benchmarking
    // 2001:2::/48 and ORCHID 2001:10::/28 and 2001:20::/28.
    if s[0] == 0x2001
        && (s[1] == 0x0000
            || s[1] == 0x0db8
            || s[1] == 0x0002
            || (s[1] & 0xfff0) == 0x0010
            || (s[1] & 0xfff0) == 0x0020)
    {
        return false;
    }
    // 6to4 2002::/16 embeds an IPv4 address.
    if s[0] == 0x2002 {
        return is_public_ipv4(embedded_ipv4(s[1], s[2]));
    }
    // NAT64 64:ff9b::/96 embeds an IPv4 address; 64:ff9b:1::/48 is local-use.
    if s[0] == 0x0064 && s[1] == 0xff9b {
        if s[2] == 0x0001 {
            return false;
        }
        if s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
            return is_public_ipv4(embedded_ipv4(s[6], s[7]));
        }
    }
    // SIIT ::ffff:0:0/96 embeds an IPv4 address, like the mapped form.
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0xffff && s[5] == 0 {
        return is_public_ipv4(embedded_ipv4(s[6], s[7]));
    }
    // Documentation 3fff::/20 and SRv6 5f00::/16.
    if s[0] == 0x3fff && (s[1] & 0xf000) == 0 {
        return false;
    }
    if s[0] == 0x5f00 {
        return false;
    }
    // IPv4-compatible ::/96.
    if s[0..6].iter().all(|&x| x == 0) {
        return false;
    }
    // Discard 100::/64.
    if s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0 {
        return false;
    }
    true
}

/// The IPv4 address stored in two IPv6 groups.
fn embedded_ipv4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
}

/// Parse a host as an IP literal, including a bracketed IPv6 literal.
fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.parse::<IpAddr>().ok()
}

/// Resolve a host to every address it has. An IP literal is returned as-is.
async fn resolve_addresses(host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    if let Some(ip) = parse_ip_literal(host) {
        return Ok(vec![ip]);
    }
    #[cfg(test)]
    if let Some(ips) = test_resolver_lookup(host) {
        return Ok(ips);
    }
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("could not resolve {host}: {e}"))?;
    let ips: Vec<IpAddr> = addrs.map(|addr| addr.ip()).collect();
    if ips.is_empty() {
        Err(format!("{host} did not resolve to an address"))
    } else {
        Ok(ips)
    }
}

/// Schemes that carry their own content and make no network request.
fn is_opaque_scheme(scheme: &str) -> bool {
    matches!(scheme, "data" | "blob" | "about")
}

/// Refuse a URL whose scheme is not http(s) or a source of local content, so
/// `file:` and friends are never fetched.
fn check_scheme(url: &url::Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        scheme if is_opaque_scheme(scheme) => Ok(()),
        scheme => Err(format!(
            "refusing a {scheme}: URL: only http and https are allowed"
        )),
    }
}

/// The first proxy environment variable that is set, if any. Servo's own
/// network stack honours these too, so they move requests off the checked path.
fn proxy_environment_variable() -> Option<String> {
    // `vars_os`, not `vars`: a non-UTF-8 variable elsewhere in the environment
    // must not panic the render.
    std::env::vars_os().find_map(|(name, value)| {
        let name = name.to_string_lossy().to_string();
        let upper = name.to_ascii_uppercase();
        (matches!(upper.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY") && !value.is_empty())
            .then_some(name)
    })
}

/// Refuse a URL whose host is not public, and return the addresses it
/// resolved to. A URL with no host, such as a `data:` URL, resolves to nothing.
async fn check_destination(url: &url::Url) -> Result<Vec<IpAddr>, String> {
    check_scheme(url)?;
    let Some(host) = url.host_str() else {
        return Ok(Vec::new());
    };
    let port = url.port_or_known_default().unwrap_or(0);
    let ips = resolve_addresses(host, port).await?;
    // A test can mark a loopback port as public, so a redirect away from it
    // can be exercised without a real public address.
    if test_public_port(port) {
        return Ok(ips);
    }
    if let Some(ip) = ips.iter().find(|ip| !is_public_address(**ip)) {
        return Err(format!(
            "refusing to fetch a non-public address for {host}: {ip} is loopback, private or \
             otherwise not a public destination"
        ));
    }
    Ok(ips)
}

/// Remove `user:pass@` from a URL, so credentials are never sent to a host.
pub fn strip_userinfo(mut url: url::Url) -> url::Url {
    if !url.username().is_empty() {
        let _ = url.set_username("");
    }
    if url.password().is_some() {
        let _ = url.set_password(None);
    }
    url
}

/// A test-only DNS override. Tests set the addresses a host resolves to
/// without touching real DNS.
#[cfg(test)]
static TEST_RESOLVER: Mutex<Option<HashMap<String, Vec<IpAddr>>>> = Mutex::new(None);

/// Ports an integration test has marked as public. See [`set_test_public_ports`].
#[cfg(feature = "test-hooks")]
static TEST_PUBLIC_PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// Mark ports as public for the address check, so an integration test can
/// point a redirect at loopback from a "public" origin. A test hook behind
/// the non-default `test-hooks` feature, not for general use.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn set_test_public_ports(ports: &[u16]) {
    if let Ok(mut guard) = TEST_PUBLIC_PORTS.lock() {
        *guard = ports.to_vec();
    }
}

#[cfg(feature = "test-hooks")]
fn test_public_port(port: u16) -> bool {
    TEST_PUBLIC_PORTS
        .lock()
        .map(|guard| guard.contains(&port))
        .unwrap_or(false)
}

#[cfg(not(feature = "test-hooks"))]
fn test_public_port(port: u16) -> bool {
    let _ = port;
    false
}

#[cfg(test)]
fn test_resolver_lookup(host: &str) -> Option<Vec<IpAddr>> {
    TEST_RESOLVER
        .lock()
        .ok()
        .and_then(|map| map.as_ref().and_then(|m| m.get(host).cloned()))
}

/// Time allowed for one page to load and settle.
pub const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest timeout [`render_page`] accepts.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// Time allowed for one request made by the page's scripts.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

const VIEWPORT_WIDTH: u32 = 1280;
const VIEWPORT_HEIGHT: u32 = 1024;
/// How often the page's text is sampled while waiting for it to settle.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(200);
/// Consecutive unchanged samples needed before the page counts as settled.
const STABLE_SAMPLES: u32 = 3;
/// Minimum time after load completes before the page can count as settled.
const MIN_SETTLE: Duration = Duration::from_secs(1);
/// The page counts as settled only after its network has been quiet this long.
/// Pages often keep content hidden until their requests come back, and a
/// framework can take a moment between loading its scripts and requesting data.
const NETWORK_QUIET: Duration = Duration::from_millis(1500);
/// A text sample unanswered for this long means the page's scripts are stuck.
const STALL_AFTER: Duration = Duration::from_secs(3);
/// Grace period beyond the job's timeout for the render thread to reply.
const REPLY_SLACK: Duration = Duration::from_secs(10);
/// Largest response body accepted for a request made on the page's behalf.
const MAX_PROXY_BODY: usize = 5 * 1024 * 1024;
/// Largest number of redirects followed for one request.
const MAX_REDIRECTS: usize = 10;
/// Response headers that describe the connection rather than the content.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
];

/// Script run in the page to read its title and visible text. `innerText`
/// follows layout, so hidden elements such as collapsed menus and dropdowns
/// are left out, along with script and style source. If `innerText` is not
/// available, it falls back to the text of a copy of the body with script,
/// style, and template content removed.
const SNAPSHOT_SCRIPT: &str = "(function () {\
    var body = document.body;\
    if (!body) { return {title: document.title, text: ''}; }\
    var text = body.innerText;\
    if (typeof text !== 'string') {\
        var copy = body.cloneNode(true);\
        copy.querySelectorAll('script, style, noscript, template').forEach(function (node) { node.remove(); });\
        text = copy.textContent;\
    }\
    return {title: document.title, text: text};\
})()";

/// A rendered page: its title and the visible text, one line per block.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedPage {
    pub title: String,
    pub text: String,
    /// Set when the page did not settle before the timeout, saying why. The
    /// text is then the last snapshot taken and may be missing content.
    pub incomplete: Option<String>,
}

struct Job {
    url: url::Url,
    timeout: Duration,
    /// Runtime for the HTTP requests made on the page's behalf.
    runtime: tokio::runtime::Handle,
    reply: mpsc::Sender<Result<RenderedPage, String>>,
}

/// The running render thread. Dropping its job sender ends the thread, and
/// joining it lets Servo shut down before the process exits.
struct RenderThread {
    jobs: mpsc::Sender<Job>,
    handle: JoinHandle<()>,
}

/// Where the renderer is in its once-per-process life.
enum Lifecycle {
    NotStarted,
    Running(RenderThread),
    /// [`shutdown_renderer`] was called. Servo cannot start again.
    ShutDown,
}

static RENDERER: Mutex<Lifecycle> = Mutex::new(Lifecycle::NotStarted);

/// Render `url` in Servo and return its title and text. The page is loaded
/// fresh every call. Errors cover startup failures, a page that never produced
/// any text, and a render thread that does not answer. A page that loads but
/// does not settle in time is returned with [`RenderedPage::incomplete`] set.
///
/// Must be called from within a Tokio runtime, which also carries the HTTP
/// requests the page's scripts make.
pub async fn render_page(url: url::Url, timeout: Duration) -> Result<RenderedPage, RenderError> {
    if timeout.is_zero() || timeout > MAX_TIMEOUT {
        return Err(RenderError::InvalidTimeout);
    }
    // One deadline for the address check and the render, so the total stays
    // inside the caller's budget.
    let deadline = Instant::now() + timeout;
    let url = strip_userinfo(url);
    if let Err(e) = check_scheme(&url) {
        return Err(RenderError::Failed(e));
    }
    if !allow_private_networks() {
        if let Some(name) = proxy_environment_variable() {
            return Err(RenderError::Failed(format!(
                "{name} is set, so requests could go through an un-checked proxy; unset it or \
                 allow private networks for local testing"
            )));
        }
        match tokio::time::timeout(time_left(deadline), check_destination(&url)).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(RenderError::Failed(e)),
            Err(_) => {
                return Err(RenderError::Failed(format!(
                    "timed out checking the address of {}",
                    url.host_str().unwrap_or("the URL")
                )));
            }
        }
    }
    let remaining = time_left(deadline);
    let runtime = tokio::runtime::Handle::current();
    let result = tokio::task::spawn_blocking(move || -> Result<RenderedPage, String> {
        let jobs = renderer()?;
        let (reply, result) = mpsc::channel();
        jobs.send(Job {
            url,
            timeout: remaining,
            runtime,
            reply,
        })
        .map_err(|_| "Servo render thread is not running".to_string())?;
        result
            .recv_timeout(remaining + REPLY_SLACK)
            .map_err(|e| format!("render did not finish: {e}"))?
    })
    .await
    .map_err(|e| format!("render task failed: {e}"))
    .and_then(|r| r);
    result.map_err(RenderError::Failed)
}

/// The time left before `deadline`, which may be zero.
fn time_left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// A human-readable budget: milliseconds under a second, whole seconds above.
fn budget_label(timeout: Duration) -> String {
    if timeout.as_secs() >= 1 {
        format!("{} seconds", timeout.as_secs())
    } else {
        format!("{} ms", timeout.as_millis())
    }
}

/// Start the render thread on first use and return a sender for its jobs.
fn renderer() -> Result<mpsc::Sender<Job>, String> {
    let mut guard = RENDERER
        .lock()
        .map_err(|_| "render state poisoned".to_string())?;
    match &*guard {
        Lifecycle::Running(thread) => return Ok(thread.jobs.clone()),
        Lifecycle::ShutDown => return Err("the renderer has been shut down".to_string()),
        Lifecycle::NotStarted => {}
    }
    let (jobs, rx) = mpsc::channel::<Job>();
    let handle = thread::Builder::new()
        .name("servo-render".into())
        .spawn(move || render_thread(rx))
        .map_err(|e| format!("failed to start Servo render thread: {e}"))?;
    let sender = jobs.clone();
    *guard = Lifecycle::Running(RenderThread { jobs, handle });
    Ok(sender)
}

/// Stop the render thread and shut Servo down. Call this before the process
/// exits: skipping it can crash during process teardown. Safe to call when
/// rendering was never started, and calling it twice is harmless. Servo runs
/// once per process, so later renders fail instead of starting it again.
pub fn shutdown_renderer() {
    // Mark the renderer shut down before waiting, so no render can start a
    // second Servo while this one stops.
    let previous = match RENDERER.lock() {
        Ok(mut guard) => std::mem::replace(&mut *guard, Lifecycle::ShutDown),
        Err(_) => return,
    };
    if let Lifecycle::Running(RenderThread { jobs, handle }) = previous {
        // Closing the channel makes the render thread leave its loop and drop Servo.
        drop(jobs);
        let _ = handle.join();
    }
}

/// Owns the Servo instance and rendering context, and renders jobs in order.
fn render_thread(jobs: mpsc::Receiver<Job>) {
    // Servo's network stack needs a process-wide TLS crypto provider. Another
    // component may already have installed one, so a failure here is ignored.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let prefs = Preferences {
        user_agent: user_agent().to_string(),
        ..Default::default()
    };
    let servo = ServoBuilder::default()
        .preferences(prefs)
        .event_loop_waker(Box::new(NoopWaker))
        .build();
    // Loads with no live webview (for example, a request from a render that
    // has already returned) go to this delegate. Dropping one would let Servo
    // fetch it unchecked, so cancel it.
    servo.set_delegate(Rc::new(CancelUnhandledLoads));

    let context =
        match SoftwareRenderingContext::new(PhysicalSize::new(VIEWPORT_WIDTH, VIEWPORT_HEIGHT)) {
            Ok(context) => Rc::new(context),
            Err(e) => {
                let message = format!("failed to create Servo rendering context: {e:?}");
                for job in jobs {
                    let _ = job.reply.send(Err(message.clone()));
                }
                return;
            }
        };
    let _ = context.make_current();

    for job in jobs {
        let result = render_one(&servo, &context, &job);
        let _ = job.reply.send(result);
    }
}

/// Cancels every load that is not tied to a live webview, which is where a
/// request from a finished render ends up.
struct CancelUnhandledLoads;

impl ServoDelegate for CancelUnhandledLoads {
    fn load_web_resource(&self, load: WebResourceLoad) {
        cancel_load(load);
    }
}

/// Servo calls `wake` from its own threads. This thread spins the event loop
/// on a fixed cadence, so it does not need to be woken.
struct NoopWaker;

impl EventLoopWaker for NoopWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(NoopWaker)
    }

    fn wake(&self) {}
}

/// What the page script last reported, and how often it has reported.
#[derive(Default)]
struct Snapshot {
    /// Which document the snapshot belongs to. Replies from an earlier
    /// document's evaluation are ignored.
    generation: u32,
    title: String,
    text: String,
    /// A script evaluation is in flight.
    pending: bool,
    /// Successful samples received so far.
    samples: u32,
}

/// Receives the page's resource requests. Every load is held until the render
/// loop has checked its destination; then it is proxied, blocked or dropped
/// (which lets Servo make it as usual).
#[derive(Default)]
struct PageDelegate {
    held: RefCell<Vec<WebResourceLoad>>,
    /// When the page last started any request.
    last_request: Cell<Option<Instant>>,
}

impl WebViewDelegate for PageDelegate {
    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        self.last_request.set(Some(Instant::now()));
        self.held.borrow_mut().push(load);
    }
}

/// Whether a request is made here instead of by Servo: a GET from a script
/// (XHR or fetch, which have no destination) to the page's own origin. The
/// request body is not available to the embedder, so other methods are left
/// to Servo, and cross-origin requests are left to Servo's CORS handling.
fn should_proxy(
    destination: Destination,
    method: &str,
    url: &url::Url,
    referrer: Option<&url::Url>,
) -> bool {
    destination == Destination::None
        && method == "GET"
        && matches!(url.scheme(), "http" | "https")
        && referrer.is_some_and(|r| r.origin() == url.origin())
}

/// A response fetched on the page's behalf.
struct ProxiedResponse {
    url: url::Url,
    status: reqwest::StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

type ProxyResult = Result<ProxiedResponse, String>;

/// Whether a redirect from `from_host` to `next` is allowed. Pure, so the
/// policy can be unit-tested in default mode.
fn redirect_decision(from_host: &str, next: &url::Url, allow_private: bool) -> Result<(), String> {
    let Some(host) = next.host_str() else {
        return Err("a redirect with no host".to_string());
    };
    if let Some(ip) = parse_ip_literal(host) {
        if !allow_private && !is_public_address(ip) {
            return Err("a redirect to a non-public address".to_string());
        }
    }
    if !allow_private && host != from_host {
        return Err("a redirect to a different host".to_string());
    }
    Ok(())
}

/// A reqwest redirect policy that turns a redirect into a private address or
/// to a different host into an error, instead of following it or handing the
/// 3xx back to the page. Only the first hop was checked, so only same-host
/// redirects are followed.
fn redirect_policy(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    let from_host = attempt
        .previous()
        .first()
        .and_then(|first| first.host_str())
        .unwrap_or_default()
        .to_string();
    let next = attempt.url().clone();
    match redirect_decision(&from_host, &next, allow_private_networks()) {
        Ok(()) => attempt.follow(),
        Err(e) => attempt.error(e),
    }
}

/// Requests made on the page's behalf that have not been answered yet.
struct Proxy {
    runtime: tokio::runtime::Handle,
    allow_private: bool,
    next_id: u64,
    /// Loads whose destination is being resolved and checked.
    checking: HashMap<u64, WebResourceLoad>,
    /// Loads whose request we make ourselves.
    in_flight: HashMap<u64, WebResourceLoad>,
    /// The spawned fetch task for each in-flight request.
    tasks: HashMap<u64, tokio::task::JoinHandle<()>>,
    /// When a request last started or finished.
    last_activity: Option<Instant>,
    check_tx: mpsc::Sender<(u64, Result<Vec<IpAddr>, String>)>,
    check_rx: mpsc::Receiver<(u64, Result<Vec<IpAddr>, String>)>,
    done_tx: mpsc::Sender<(u64, ProxyResult)>,
    done_rx: mpsc::Receiver<(u64, ProxyResult)>,
}

impl Proxy {
    fn new(runtime: tokio::runtime::Handle, allow_private: bool) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let (check_tx, check_rx) = mpsc::channel();
        Self {
            runtime,
            allow_private,
            next_id: 0,
            checking: HashMap::new(),
            in_flight: HashMap::new(),
            tasks: HashMap::new(),
            last_activity: None,
            check_tx,
            check_rx,
            done_tx,
            done_rx,
        }
    }

    /// No check or request is outstanding, and none has started or finished
    /// for `quiet`.
    fn is_idle(&self, now: Instant, quiet: Duration) -> bool {
        self.checking.is_empty()
            && self.in_flight.is_empty()
            && self
                .last_activity
                .is_none_or(|t| now.duration_since(t) >= quiet)
    }

    /// Check a load's destination, then proxy it or hand it back to Servo.
    fn submit(&mut self, servo: &Servo, load: WebResourceLoad) {
        let url = load.request.url.clone().into_url();
        // Only http(s), data, blob and about are ever allowed; anything else
        // is refused before Servo can fetch it.
        if let Err(e) = check_scheme(&url) {
            tracing::debug!(host = %url.host_str().unwrap_or_default(), error = %e, "blocked a non-http scheme");
            cancel_load(load);
            return;
        }
        if self.allow_private {
            self.dispatch(servo, load, &[]);
            return;
        }
        // A redirect hop Servo would follow: the embedder only sees the
        // original URL, never the target, so make the request ourselves and
        // check every hop. A non-GET hop is refused outright.
        if load.request.is_redirect {
            let method = load.request.method.as_str();
            if method == "GET" || method == "HEAD" {
                self.start_redirect(load);
            } else {
                cancel_load(load);
            }
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.checking.insert(id, load);
        self.last_activity = Some(Instant::now());
        let host = url.host_str().unwrap_or_default().to_string();
        let tx = self.check_tx.clone();
        let handle = self.runtime.spawn(async move {
            let result = match tokio::time::timeout(REQUEST_TIMEOUT, check_destination(&url)).await
            {
                Ok(result) => result,
                Err(_) => Err(format!("timed out checking {host}")),
            };
            let _ = tx.send((id, result));
        });
        self.tasks.insert(id, handle);
    }

    /// Hand finished checks back. A refused load becomes a network error in
    /// the page; an allowed one is proxied or left to Servo.
    fn deliver_checks(&mut self, servo: &Servo) {
        while let Ok((id, result)) = self.check_rx.try_recv() {
            let Some(load) = self.checking.remove(&id) else {
                continue;
            };
            self.tasks.remove(&id);
            self.last_activity = Some(Instant::now());
            match result {
                Ok(addrs) => self.dispatch(servo, load, &addrs),
                Err(e) => {
                    tracing::debug!(host = %load.request.url.as_url().host_str().unwrap_or_default(), error = %e, "blocked a non-public request");
                    cancel_load(load);
                }
            }
        }
    }

    /// Make the request here when it is a same-origin script GET, otherwise
    /// drop the load and let Servo make it.
    fn dispatch(&mut self, servo: &Servo, load: WebResourceLoad, addrs: &[IpAddr]) {
        let request = &load.request;
        if should_proxy(
            request.destination,
            request.method.as_str(),
            request.url.as_url(),
            request.referrer_url.as_ref().map(|u| u.as_url()),
        ) {
            self.start(servo, load, addrs);
        }
    }

    /// Start a request for `load` with the cookies Servo holds for its URL.
    fn start(&mut self, servo: &Servo, load: WebResourceLoad, addrs: &[IpAddr]) {
        let request = &load.request;
        let url = strip_userinfo(request.url.clone().into_url());
        let mut headers = request.headers.clone();
        let cookies = servo
            .site_data_manager()
            .cookies_for_url(url.clone(), CookieSource::HTTP);
        if !cookies.is_empty() {
            let pairs: Vec<String> = cookies
                .iter()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect();
            if let Ok(value) = HeaderValue::from_str(&pairs.join("; ")) {
                headers.insert(COOKIE, value);
            }
        }
        if let Some(referrer) = request.referrer_url.as_ref().map(|u| u.as_url()) {
            if let Ok(value) = HeaderValue::from_str(referrer.as_str()) {
                headers.insert(REFERER, value);
            }
        }
        if let Ok(value) = HeaderValue::from_str(user_agent()) {
            headers.insert(UA_HEADER, value);
        }

        let client = match pinned_client(&url, addrs, true) {
            Ok(client) => client,
            Err(e) => {
                tracing::debug!(host = %url.host_str().unwrap_or_default(), error = %e, "blocked a request");
                cancel_load(load);
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.in_flight.insert(id, load);
        self.last_activity = Some(Instant::now());
        let done = self.done_tx.clone();
        let handle = self.runtime.spawn(async move {
            let result = tokio::time::timeout(REQUEST_TIMEOUT, fetch(client, url, headers))
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "no response within {} seconds",
                        REQUEST_TIMEOUT.as_secs()
                    ))
                });
            let _ = done.send((id, result));
        });
        self.tasks.insert(id, handle);
    }

    /// Follow a redirect hop ourselves, checking each hop, and hand the render
    /// the final response.
    fn start_redirect(&mut self, load: WebResourceLoad) {
        let request = &load.request;
        let url = strip_userinfo(request.url.clone().into_url());
        let mut headers = request.headers.clone();
        if let Some(referrer) = request.referrer_url.as_ref().map(|u| u.as_url()) {
            if let Ok(value) = HeaderValue::from_str(referrer.as_str()) {
                headers.insert(REFERER, value);
            }
        }
        if let Ok(value) = HeaderValue::from_str(user_agent()) {
            headers.insert(UA_HEADER, value);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.in_flight.insert(id, load);
        self.last_activity = Some(Instant::now());
        let done = self.done_tx.clone();
        let handle = self.runtime.spawn(async move {
            let result = tokio::time::timeout(REQUEST_TIMEOUT, fetch_redirect_chain(url, headers))
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "no response within {} seconds",
                        REQUEST_TIMEOUT.as_secs()
                    ))
                });
            let _ = done.send((id, result));
        });
        self.tasks.insert(id, handle);
    }

    /// Cancel every load that is still waiting, so none is dropped and
    /// released to Servo without a decision.
    fn cancel_all(&mut self) {
        for (_, load) in self.checking.drain() {
            cancel_load(load);
        }
        for (_, load) in self.in_flight.drain() {
            cancel_load(load);
        }
        for (_, handle) in self.tasks.drain() {
            handle.abort();
        }
    }

    /// Hand finished requests back to Servo. A failed request becomes a
    /// network error in the page.
    fn deliver_finished(&mut self, servo: &Servo) {
        while let Ok((id, result)) = self.done_rx.try_recv() {
            let Some(load) = self.in_flight.remove(&id) else {
                continue;
            };
            self.tasks.remove(&id);
            self.last_activity = Some(Instant::now());
            match result {
                Ok(response) => {
                    store_cookies(servo, &response);
                    let mut headers = response.headers;
                    for name in HOP_BY_HOP {
                        headers.remove(*name);
                    }
                    let reason = response
                        .status
                        .canonical_reason()
                        .unwrap_or("")
                        .as_bytes()
                        .to_vec();
                    let mut intercepted = load.intercept(
                        WebResourceResponse::new(response.url)
                            .headers(headers)
                            .status_code(response.status)
                            .status_message(reason),
                    );
                    if !response.body.is_empty() {
                        intercepted.send_body_data(response.body);
                    }
                    intercepted.finish();
                }
                Err(e) => {
                    tracing::debug!(host = %load.request.url.as_url().host_str().unwrap_or_default(), error = %e, "page request failed");
                    cancel_load(load);
                }
            }
        }
    }
}

/// Follow `url`'s redirects ourselves, checking and pinning every hop. The
/// first hop is requested twice: Servo already made it and its redirect is
/// the load being answered.
async fn fetch_redirect_chain(url: url::Url, headers: HeaderMap) -> ProxyResult {
    let mut url = url;
    for _ in 0..MAX_REDIRECTS {
        let addrs = check_destination(&url).await?;
        let client = pinned_client(&url, &addrs, false)?;
        let response = fetch(client, url.clone(), headers.clone()).await?;
        if !response.status.is_redirection() {
            return Ok(response);
        }
        let location = response
            .headers
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| "a redirect response had no Location".to_string())?;
        let next = url.join(location).map_err(|e| e.to_string())?;
        check_scheme(&next)?;
        if !test_public_port(next.port_or_known_default().unwrap_or(0)) {
            redirect_decision(url.host_str().unwrap_or_default(), &next, false)?;
        }
        url = next;
    }
    Err(format!("more than {MAX_REDIRECTS} redirects"))
}

/// Build the client for one proxied request, pinned to the addresses the
/// check resolved so a second answer cannot move the connection to a private
/// host.
fn pinned_client(
    url: &url::Url,
    addrs: &[IpAddr],
    follow: bool,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(REQUEST_TIMEOUT)
        .no_proxy()
        .redirect(if follow {
            reqwest::redirect::Policy::custom(redirect_policy)
        } else {
            reqwest::redirect::Policy::none()
        });
    if let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) {
        let sockets: Vec<std::net::SocketAddr> = addrs
            .iter()
            .map(|ip| std::net::SocketAddr::new(*ip, port))
            .collect();
        if !sockets.is_empty() {
            builder = builder.resolve_to_addrs(host, &sockets);
        }
    }
    builder
        .build()
        .map_err(|e| format!("could not build the proxy client: {e}"))
}

/// Cancel a load that was not allowed to proceed. Dropping one instead would
/// let Servo fetch it without a check.
fn cancel_load(load: WebResourceLoad) {
    let url = load.request.url.clone().into_url();
    load.intercept(WebResourceResponse::new(url)).cancel();
}

async fn fetch(client: reqwest::Client, url: url::Url, headers: HeaderMap) -> ProxyResult {
    let mut response = client
        .get(url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let url = response.url().clone();
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > MAX_PROXY_BODY {
            return Err(format!("response body exceeds {MAX_PROXY_BODY} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(ProxiedResponse {
        url,
        status,
        headers,
        body,
    })
}

/// Servo stores cookies only from responses it fetched itself, so the cookies
/// set by a proxied response are stored here.
fn store_cookies(servo: &Servo, response: &ProxiedResponse) {
    for value in response.headers.get_all(SET_COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        if let Ok(cookie) = cookie::Cookie::parse(text.to_string()) {
            servo.site_data_manager().set_cookie_for_url(
                response.url.clone(),
                cookie.into_owned(),
                None,
            );
        }
    }
}

/// Forget everything earlier renders left behind: cookies, web storage and
/// cached responses. Each render then behaves like a first visit, so a site
/// whose bot protection flagged one visit does not block every later one.
fn reset_site_state(servo: &Servo) {
    let sites = servo.site_data_manager().site_data(StorageType::all());
    let names: Vec<String> = sites.iter().map(|site| site.name()).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    servo
        .site_data_manager()
        .clear_site_data(&names, StorageType::Local | StorageType::Session);
    servo.site_data_manager().clear_cookies(None);
    servo.network_manager().clear_cache();
}

fn render_one(
    servo: &Servo,
    context: &Rc<SoftwareRenderingContext>,
    job: &Job,
) -> Result<RenderedPage, String> {
    reset_site_state(servo);
    let rendering: Rc<dyn RenderingContext> = context.clone();
    let delegate = Rc::new(PageDelegate::default());
    let webview: WebView = WebViewBuilder::new(servo, rendering)
        .url(job.url.clone())
        .delegate(delegate.clone())
        .build();
    let mut proxy = Proxy::new(job.runtime.clone(), allow_private_networks());

    let snapshot = Rc::new(RefCell::new(Snapshot::default()));
    let deadline = Instant::now() + job.timeout;
    let mut loaded_at: Option<Instant> = None;
    let mut last_sample: Option<Instant> = None;
    let mut seen_samples = 0;
    let mut stable = 0;
    let mut last_text: Option<String> = None;
    let mut settled;

    loop {
        // Servo's callbacks run inside spin_event_loop, so no RefCell borrow is
        // held across this call.
        servo.spin_event_loop();
        let now = Instant::now();

        let held: Vec<WebResourceLoad> = delegate.held.borrow_mut().drain(..).collect();
        for load in held {
            proxy.submit(servo, load);
        }
        proxy.deliver_checks(servo);
        proxy.deliver_finished(servo);

        let status = webview.load_status();
        if status == LoadStatus::Complete {
            loaded_at.get_or_insert(now);
        } else if loaded_at.is_some() {
            // A new document started loading after the last one finished, for
            // example when a bot check reloads the page. Wait for it instead.
            // Its text, and any evaluation still running in it, belong to
            // the old document, so start over.
            loaded_at = None;
            stable = 0;
            last_text = None;
            last_sample = None;
            seen_samples = 0;
            let generation = snapshot.borrow().generation.wrapping_add(1);
            *snapshot.borrow_mut() = Snapshot {
                generation,
                ..Snapshot::default()
            };
        }

        // Sample as soon as the document exists, so a page that never finishes
        // loading still yields its text. Only a loaded page can settle.
        if status != LoadStatus::Started {
            let due = last_sample.is_none_or(|t| now.duration_since(t) >= SAMPLE_INTERVAL);
            if due && !snapshot.borrow().pending {
                last_sample = Some(now);
                snapshot.borrow_mut().pending = true;
                let generation = snapshot.borrow().generation;
                let shared = snapshot.clone();
                webview.evaluate_javascript(SNAPSHOT_SCRIPT, move |result| {
                    let mut snap = shared.borrow_mut();
                    if snap.generation != generation {
                        return;
                    }
                    snap.pending = false;
                    if let Ok(JSValue::Object(fields)) = result {
                        snap.title = string_field(&fields, "title");
                        snap.text = string_field(&fields, "text");
                        snap.samples += 1;
                    }
                });
            }

            // Count a sample only once it has arrived, and only compare texts
            // from distinct samples.
            let snap = snapshot.borrow();
            if snap.samples > seen_samples {
                seen_samples = snap.samples;
                // Compare the sanitized text: a page that is only control
                // characters has no text to settle on.
                let sample = sanitize_control(&snap.text);
                if last_text.as_deref() == Some(sample.as_str()) {
                    stable += 1;
                } else {
                    stable = 0;
                    last_text = Some(sample);
                }
            }
        }

        let network_quiet = proxy.is_idle(now, NETWORK_QUIET)
            && delegate
                .last_request
                .get()
                .is_none_or(|t| now.duration_since(t) >= NETWORK_QUIET);
        // An empty page is not settled: a bot check may be about to reload it.
        let has_text = last_text.as_deref().is_some_and(|t| !t.trim().is_empty());
        settled = stable >= STABLE_SAMPLES
            && has_text
            && network_quiet
            && loaded_at.is_some_and(|loaded| now.duration_since(loaded) >= MIN_SETTLE);
        if settled || now >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let snap = snapshot.borrow();
    let stalled = snap.pending
        && last_sample.is_some_and(|sent| Instant::now().duration_since(sent) >= STALL_AFTER);
    let secs = budget_label(job.timeout);
    if snap.samples == 0 {
        drop(snap);
        cancel_pending(&mut proxy, &delegate);
        return Err(if stalled {
            format!("page scripts stopped responding before any text could be read (waited {secs})")
        } else {
            format!("page did not finish loading within {secs}")
        });
    }
    let text = normalize_text(&sanitize_control(&snap.text));
    let incomplete = if settled {
        None
    } else if text.is_empty() {
        Some(format!(
            "the page showed no readable text after {secs}. The site may be blocking \
             automated browsers, or the page may need a login. Fetching it again is unlikely to help."
        ))
    } else if stalled {
        Some(
            "the page's scripts stopped responding, probably waiting on a request that never \
             finished. This text was captured before that and may be missing content."
                .to_string(),
        )
    } else if loaded_at.is_none() {
        Some(format!(
            "the page was still loading after {secs}. Some content may be missing."
        ))
    } else if !proxy.in_flight.is_empty() {
        Some(format!(
            "the page was still waiting on {} of its own requests after {secs}. Some content may be missing.",
            proxy.in_flight.len()
        ))
    } else {
        Some(format!(
            "the page was still changing after {secs}. Some content may be missing."
        ))
    };
    let title = sanitize_control(&snap.title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    drop(snap);
    cancel_pending(&mut proxy, &delegate);
    Ok(RenderedPage {
        title,
        text,
        incomplete,
    })
}

/// Cancel every load that is still waiting, in the proxy and in the delegate,
/// so none is dropped and released to Servo without a decision.
fn cancel_pending(proxy: &mut Proxy, delegate: &PageDelegate) {
    proxy.cancel_all();
    let held: Vec<WebResourceLoad> = delegate.held.borrow_mut().drain(..).collect();
    for load in held {
        cancel_load(load);
    }
}

/// Collapse whitespace within each line and drop empty lines.
fn normalize_text(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove C0 and C1 control characters other than newline and tab, and the
/// bidi overrides and isolates, so a page cannot inject terminal escapes or
/// reorder terminal output. Vertical tab, form feed and NEL become spaces so
/// they still separate words; carriage return is removed.
fn sanitize_control(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' | '\t' => Some(c),
            // Keep word separators, so a page's layout does not glue words.
            '\u{0b}' | '\u{0c}' | '\u{85}' => Some(' '),
            // Bidi overrides and isolates can reorder a caller's output.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => None,
            _ if c.is_control() => None,
            _ => Some(c),
        })
        .collect()
}

fn string_field(fields: &HashMap<String, JSValue>, key: &str) -> String {
    match fields.get(key) {
        Some(JSValue::String(value)) => value.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    /// The visible text is written by a script after load, so the raw HTML
    /// only contains "loading". A hidden element must not show up.
    const SPA: &str = "<html><head><title>SPA Test</title></head><body>\
        <div id=\"app\">loading</div>\
        <div style=\"display:none\">hidden-dropdown-marker</div>\
        <script>setTimeout(function(){document.getElementById('app').textContent='rendered-by-javascript-marker ua:'+navigator.userAgent;},300);</script>\
        </body></html>";

    /// Makes a synchronous XHR to an endpoint that never answers, then one
    /// that echoes the request's cookies and sets a new one.
    const SYNC_XHR: &str = "<html><head><title>Sync XHR</title></head><body>\
        <div id=\"out\">before</div>\
        <script>\
        document.cookie = 'page=1';\
        var out = [];\
        try { var x = new XMLHttpRequest(); x.open('GET', '/hang', false); x.send(); out.push('hang-answered'); }\
        catch (e) { out.push('hang-failed'); }\
        var y = new XMLHttpRequest(); y.open('GET', '/cookies', false); y.send();\
        out.push('server-saw:' + y.responseText);\
        out.push('page-sees:' + document.cookie);\
        document.getElementById('out').textContent = out.join(' ');\
        </script></body></html>";

    /// Reports the cookies and storage left by an earlier visit, then leaves
    /// some of its own for the next visit to find.
    const STATE: &str = "<html><head><title>State</title></head><body>\
        <div id=\"out\">before</div>\
        <script>\
        document.getElementById('out').textContent = 'cookie:[' + document.cookie + '] stored:[' \
            + localStorage.getItem('left') + '] session:[' + sessionStorage.getItem('left') + ']';\
        document.cookie = 'left=1; Max-Age=3600';\
        localStorage.setItem('left', '1');\
        sessionStorage.setItem('left', '1');\
        </script></body></html>";

    /// A page whose title and text carry terminal escapes: ESC, the C1 CSI
    /// (U+009B) and an OSC.
    const ESCAPES: &str = "<html><head><title>Escapes</title></head><body>\
        <div id='out'>before</div>\
        <script>\
        var esc = String.fromCharCode(27), bel = String.fromCharCode(7), csi = String.fromCharCode(155);\
        document.title = 'esc' + esc + '[31mred' + esc + '[0m';\
        document.getElementById('out').textContent = 'a' + esc + '[2Jb' + esc + ']0;title' + bel + 'c' + csi + '31md';\
        </script></body></html>";

    /// A page whose only text is an escape character.
    const ONLY_ESCAPE: &str = "<html><head><title>Only esc</title></head><body>\
        <div id='out'>x</div>\
        <script>document.getElementById('out').textContent = String.fromCharCode(27);</script>\
        </body></html>";

    /// A page whose same-origin XHR is redirected to a private address.
    const REDIRECT: &str = "<html><head><title>Redirect</title></head><body>\
        <div id=\"out\">before</div>\
        <script>\
        var x = new XMLHttpRequest();\
        x.open('GET', '/redirect', true);\
        x.onload = function () { document.getElementById('out').textContent = 'redirect-followed'; };\
        x.onerror = function () { document.getElementById('out').textContent = 'redirect-blocked'; };\
        x.send();\
        </script></body></html>";

    /// Serve `page` at `/`. `/hang` never answers, and `/cookies` echoes the
    /// request's Cookie header and sets `server=2`.
    fn serve(page: &'static str) -> url::Url {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                thread::spawn(move || handle(stream, page));
            }
        });
        url::Url::parse(&format!("http://{addr}/")).unwrap()
    }

    fn handle(mut stream: TcpStream, page: &str) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        let _ = reader.read_line(&mut request_line);
        let mut cookies = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("cookie") {
                    cookies = value.trim().to_string();
                }
            }
        }
        let path = request_line.split_whitespace().nth(1).unwrap_or("/");
        let (status, content_type, extra, body) = match path {
            "/hang" => {
                thread::sleep(Duration::from_secs(600));
                return;
            }
            "/cookies" => (
                "200 OK",
                "text/plain",
                "Set-Cookie: server=2; Path=/\r\n",
                cookies,
            ),
            // Redirects to a private address, which the proxy must refuse.
            "/redirect" => (
                "302 Found",
                "text/plain",
                "Location: http://127.0.0.1:1/\r\n",
                String::new(),
            ),
            _ => ("200 OK", "text/html; charset=utf-8", "", page.to_string()),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra}\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
    }

    // Servo can be started only once per process, so all render scenarios run
    // in one test.
    #[tokio::test(flavor = "multi_thread")]
    async fn renders_pages() {
        // A proxy in the environment would route Servo's own local requests
        // through it, so clear it before the first render.
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
        let config = RendererConfig {
            user_agent: "cherrypi-fetch-test/1.0".into(),
            // The local test servers listen on 127.0.0.1.
            allow_private_networks: true,
        };
        assert_eq!(
            configure(RendererConfig {
                user_agent: "bad\nagent".into(),
                allow_private_networks: true,
            }),
            Err(RenderError::InvalidUserAgent)
        );
        assert_eq!(configure(config.clone()), Ok(()));
        assert_eq!(configure(config), Ok(()), "same settings again are fine");
        assert_eq!(
            configure(RendererConfig::default()),
            Err(RenderError::AlreadyConfigured)
        );

        let page = render_page(serve(SPA), Duration::from_secs(60))
            .await
            .expect("render should succeed");
        assert_eq!(page.title, "SPA Test");
        assert_eq!(page.incomplete, None);
        assert!(
            page.text.contains("rendered-by-javascript-marker"),
            "text was: {:?}",
            page.text
        );
        assert!(
            page.text.contains("ua:cherrypi-fetch-test/1.0"),
            "configured user agent not used: {:?}",
            page.text
        );
        assert!(
            !page.text.contains("setTimeout"),
            "script source leaked: {:?}",
            page.text
        );
        assert!(
            !page.text.contains("hidden-dropdown-marker"),
            "hidden text leaked: {:?}",
            page.text
        );

        let started = Instant::now();
        let page = render_page(serve(SYNC_XHR), Duration::from_secs(60))
            .await
            .expect("render should succeed");
        assert!(
            started.elapsed() < REQUEST_TIMEOUT + Duration::from_secs(15),
            "render took {:?}",
            started.elapsed()
        );
        assert_eq!(page.incomplete, None, "text was: {:?}", page.text);
        assert!(
            page.text.contains("hang-failed"),
            "text was: {:?}",
            page.text
        );
        assert!(
            page.text.contains("server-saw:page=1"),
            "text was: {:?}",
            page.text
        );
        assert!(page.text.contains("server=2"), "text was: {:?}", page.text);

        // Each render starts clean: a site that flags one visit (as bot
        // protection does) must not poison the next.
        let state = serve(STATE);
        for visit in 1..=2 {
            let page = render_page(state.clone(), Duration::from_secs(60))
                .await
                .expect("render should succeed");
            assert!(
                page.text.contains("cookie:[] stored:[null] session:[null]"),
                "visit {visit} saw earlier state: {:?}",
                page.text
            );
        }

        // A page can put terminal escape sequences into its title and text.
        // The library strips the control characters before returning them.
        let page = render_page(serve(ESCAPES), Duration::from_secs(60))
            .await
            .expect("render should succeed");
        let has_control = |s: &str| s.chars().any(|c| c.is_control() && c != '\n' && c != '\t');
        assert!(
            !has_control(&page.title),
            "title kept a control character: {:?}",
            page.title
        );
        assert!(
            !has_control(&page.text),
            "text kept a control character: {:?}",
            page.text
        );
        assert_eq!(page.title, "esc[31mred[0m");
        assert!(
            page.text.contains("a[2Jb]0;titlec31md"),
            "text was: {:?}",
            page.text
        );

        // A page whose text sanitizes to nothing has no text to settle on, so
        // it comes back incomplete instead of as an empty success.
        let page = render_page(serve(ONLY_ESCAPE), Duration::from_secs(3))
            .await
            .expect("render should succeed");
        assert_eq!(page.text, "");
        assert!(
            page.incomplete.is_some(),
            "an ESC-only page should be incomplete: {page:?}"
        );

        // A same-origin script GET that redirects to a private address is
        // refused.
        let page = render_page(serve(REDIRECT), Duration::from_secs(20))
            .await
            .expect("render should succeed");
        assert!(
            page.text.contains("redirect-blocked"),
            "text was: {:?}",
            page.text
        );

        let url = url::Url::parse("http://127.0.0.1:9/").unwrap();
        for bad in [
            Duration::ZERO,
            MAX_TIMEOUT + Duration::from_secs(1),
            Duration::MAX,
        ] {
            assert_eq!(
                render_page(url.clone(), bad).await,
                Err(RenderError::InvalidTimeout)
            );
        }

        shutdown_renderer();
        shutdown_renderer();
        let after = render_page(url, Duration::from_secs(5)).await;
        assert!(
            matches!(&after, Err(RenderError::Failed(e)) if e.contains("shut down")),
            "{after:?}"
        );
    }

    #[test]
    fn proxies_only_same_origin_script_gets() {
        let page = url::Url::parse("https://example.com/page").unwrap();
        let same = url::Url::parse("https://example.com/api").unwrap();
        let other = url::Url::parse("https://other.example/api").unwrap();
        assert!(should_proxy(Destination::None, "GET", &same, Some(&page)));
        assert!(!should_proxy(Destination::None, "POST", &same, Some(&page)));
        assert!(!should_proxy(
            Destination::Script,
            "GET",
            &same,
            Some(&page)
        ));
        assert!(!should_proxy(Destination::None, "GET", &other, Some(&page)));
        assert!(!should_proxy(Destination::None, "GET", &same, None));
    }

    #[test]
    fn allows_only_http_and_opaque_schemes() {
        for url in [
            "http://example.com/",
            "https://example.com/",
            "data:text/plain,hi",
            "blob:https://example.com/x",
            "about:blank",
        ] {
            assert!(
                check_scheme(&url::Url::parse(url).unwrap()).is_ok(),
                "{url} should be allowed"
            );
        }
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/",
            "ws://example.com/",
        ] {
            assert!(
                check_scheme(&url::Url::parse(url).unwrap()).is_err(),
                "{url} should be refused"
            );
        }
    }

    #[test]
    fn redirect_decision_allows_public_same_host_only() {
        let private = url::Url::parse("http://127.0.0.1:9/").unwrap();
        assert!(redirect_decision("example.com", &private, false).is_err());
        assert!(redirect_decision("example.com", &private, true).is_ok());

        let other = url::Url::parse("http://other.example/").unwrap();
        assert!(redirect_decision("example.com", &other, false).is_err());
        assert!(redirect_decision("example.com", &other, true).is_ok());

        let same = url::Url::parse("http://example.com/next").unwrap();
        assert!(redirect_decision("example.com", &same, false).is_ok());

        let no_host = url::Url::parse("data:text/plain,hi").unwrap();
        assert!(redirect_decision("example.com", &no_host, false).is_err());
    }

    #[test]
    fn normalizes_text_lines() {
        assert_eq!(normalize_text("  a   b \n\n \t\n c\n"), "a b\nc");
    }

    #[tokio::test]
    async fn blocks_a_host_that_resolves_to_a_private_address() {
        *TEST_RESOLVER.lock().unwrap() = Some(HashMap::from([(
            "evil.test".to_string(),
            vec!["127.0.0.1".parse().unwrap()],
        )]));
        let url = url::Url::parse("http://evil.test/").unwrap();
        let result = check_destination(&url).await;
        *TEST_RESOLVER.lock().unwrap() = None;
        assert!(result.is_err(), "a host on 127.0.0.1 must be refused");
    }

    #[test]
    fn classifies_public_addresses() {
        for addr in [
            "93.184.216.34",
            "8.8.8.8",
            "172.32.0.1",
            "2606:4700:4700::1111",
            // NAT64 and 6to4 addresses embedding a public IPv4 are public.
            "64:ff9b::8.8.8.8",
            "2002:0808:0808::",
        ] {
            assert!(
                is_public_address(addr.parse().unwrap()),
                "{addr} should be public"
            );
        }
        for addr in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "127.1.2.3",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            // IPv6 transition and other reserved forms.
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "2002:c0a8:101::",
            "2001::1",
            "fec0::1",
            "::7f00:1",
            "100::1",
            // SIIT, benchmarking, ORCHID, RFC 9637 documentation and SRv6.
            "::ffff:0:127.0.0.1",
            "::ffff:0:a9fe:a9fe",
            "2001:2::1",
            "2001:10::1",
            "2001:20::1",
            "3fff::1",
            "5f00::1",
        ] {
            assert!(
                !is_public_address(addr.parse().unwrap()),
                "{addr} should be blocked"
            );
        }
    }

    #[test]
    fn strips_control_characters_but_keeps_newlines_and_tabs() {
        assert_eq!(
            sanitize_control("a\u{1b}[31mb\u{9b}c\td\ne"),
            "a[31mbc\td\ne"
        );
        // VT, FF and NEL separate words; CR is removed.
        assert_eq!(sanitize_control("a\u{b}b\u{c}c\u{85}d"), "a b c d");
        assert_eq!(sanitize_control("a\rb"), "ab");
        // Bidi overrides and isolates are removed.
        assert_eq!(sanitize_control("a\u{202e}b\u{2066}c"), "abc");
        // Every other C0, DEL and C1 code point is removed.
        for code in (0x00..=0x1f).chain(0x7f..=0x9f) {
            let c = char::from_u32(code).unwrap();
            if c == '\n' || c == '\t' {
                continue;
            }
            let expected = match c {
                '\u{b}' | '\u{c}' | '\u{85}' => "x y",
                _ => "xy",
            };
            assert_eq!(sanitize_control(&format!("x{c}y")), expected, "{code:#x}");
        }
        // Non-ASCII text is untouched.
        assert_eq!(
            sanitize_control("a\u{a0}b\u{e9}c\u{3042}d"),
            "a\u{a0}b\u{e9}c\u{3042}d"
        );
    }
}
