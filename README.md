# cherrypi-fetch

A tool that humans and agents can use to fetch data from most public pages,
even SPAs. It renders JavaScript-heavy pages in [Servo](https://servo.org) and
returns their title and visible text, as a library or a command-line tool. No
Chrome, Chromium or V8 is involved. Originally for the CherryPi AI agent
harness.

## Install

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install --locked cherrypi-fetch    # build from source, with the locked dependency versions
```

`cargo binstall` needs [cargo-binstall](https://github.com/cargo-bins/cargo-binstall)
and fetches the prebuilt archive from the GitHub release. Prebuilt archives
exist for x86_64 Linux (glibc) and Apple-silicon macOS only; other targets
fall back to a source build, which needs the
[system packages](#building-from-source).

Or download the archive for your platform from the
[Releases](https://github.com/kerryhatcher/CherryPi-Fetch/releases) page and
check its checksum:

```sh
VERSION=0.1.0
TARGET=x86_64-unknown-linux-gnu   # or aarch64-apple-darwin
curl -LO "https://github.com/kerryhatcher/CherryPi-Fetch/releases/download/v${VERSION}/cherrypi-fetch-${VERSION}-${TARGET}.tar.gz"
curl -LO "https://github.com/kerryhatcher/CherryPi-Fetch/releases/download/v${VERSION}/cherrypi-fetch-${VERSION}-${TARGET}.tar.gz.sha256"
sha256sum -c "cherrypi-fetch-${VERSION}-${TARGET}.tar.gz.sha256"   # shasum -a 256 -c on macOS
tar -xzf "cherrypi-fetch-${VERSION}-${TARGET}.tar.gz"
./cherrypi-fetch --help
```

The archive holds the `cherrypi-fetch` binary, `LICENSE` and `README.md` at
its root.

## CLI

```sh
cherrypi-fetch https://example.com/
cherrypi-fetch --json --max-chars 20000 https://a.example/ https://b.example/
```

| Option | Default | Meaning |
|---|---|---|
| `--timeout <secs>` | 20 | Time allowed for each page to load and settle (1–600) |
| `--user-agent <str>` | Windows Firefox 160 | User-Agent for the page and its requests |
| `--json` | off | One JSON object per URL (JSON Lines) instead of text |
| `--max-chars <n>` | none | Keep at most this many characters of each page's text |
| `--allow-private` | off | Allow loopback, private and link-local destinations (local testing) |

URLs are rendered one after another. Each one starts with no cookies, storage
or cache left by the ones before it.

Page data goes to stdout and diagnostics to stderr (set `RUST_LOG` for more).
Each `--json` record looks like this:

```json
{"url":"…","ok":true,"title":"…","text":"…","incomplete":null,"truncated":false,"error":null,"elapsed_ms":9784}
```

`incomplete` says why when a page did not settle in time; its text is then the
last snapshot taken. A failed record has `ok:false`, with `title` and `text`
`null` and the reason in `error`.

**Exit status:** 0 when every page rendered fully; 1 when at least one failed
(even if others were incomplete) or the arguments were bad; 2 when at least
one was incomplete and none failed.

## MCP server

Run `cherrypi-fetch mcp` as a stdio MCP server. Its `fetch_page` tool renders
HTTP and HTTPS pages and returns their title and visible text. Private and
loopback addresses are blocked by default; for local testing, start the server
with `cherrypi-fetch mcp --allow-private`.

To smoke-test the tool list with MCP Inspector:

```sh
npx @modelcontextprotocol/inspector --cli cherrypi-fetch mcp --method tools/list
```

## Library

```rust
// Cargo.toml: cherrypi-fetch = "0.1", url = "2"
async fn demo() -> Result<(), cherrypi_fetch::RenderError> {
    cherrypi_fetch::configure(cherrypi_fetch::RendererConfig {
        user_agent: "MyTool/1.0".into(),
        ..Default::default()
    })?; // optional; call before the first render
    let url = url::Url::parse("https://example.com/").unwrap();
    let result = cherrypi_fetch::render_page(url, cherrypi_fetch::RENDER_TIMEOUT).await;
    cherrypi_fetch::shutdown_renderer(); // before the process exits, on every path
    let page = result?;
    println!("{}\n{}", page.title, page.text);
    Ok(())
}
```

Servo runs once per process, on its own thread, so the settings apply to the
whole process and are fixed after the first render. After
`shutdown_renderer`, renders fail instead of starting Servo again.
`render_page` must be called from within a Tokio runtime. Build with
`default-features = false` to leave out the CLI's dependencies.

## Building from source

Servo needs a C/C++ toolchain, CMake, Clang and a GL stack. On Debian and
Ubuntu, the packages CI installs on `ubuntu-24.04` are:

```sh
sudo apt-get update
sudo apt-get install -y \
    build-essential cmake pkg-config clang libclang-dev \
    libfontconfig1-dev libegl1-mesa-dev libegl1 libgl1-mesa-dri \
    fonts-dejavu-core
```

On macOS, install the Xcode Command Line Tools and CMake
(`brew install cmake`). CI's macOS job installs nothing beyond the Rust
toolchain, because the runner image already includes its build tools.

With those installed, `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`,
`cargo build --no-default-features --locked` and
`cargo publish --dry-run --locked` work (the same checks CI runs). The render
tests need a GL implementation; on a headless machine set
`LIBGL_ALWAYS_SOFTWARE=1` so Mesa uses its software rasterizer.

## Why the default User-Agent claims Windows

Some sites never answer requests from desktop Linux user agents. Simbli
(eboardsolutions.com) was one in CherryPi: a page there waits forever on a
synchronous request. To keep a page from stalling, the same-origin GET
requests its scripts make (XHR and `fetch`) are proxied with an 8-second
timeout. Other requests are left to Servo. A render that still stalls ends at
its timeout: if it captured some text it is returned as incomplete; if it
captured no text, it fails (`ok:false`, exit 1).

## Limitations

- **Private addresses are refused by default.** Only `http` and `https` are
  fetched (`data`, `blob` and `about` are allowed locally; everything else,
  including `file:`, is refused). The page and every resource it loads must
  resolve to a public address. Loopback, private and link-local addresses
  (including cloud metadata services such as 169.254.169.254) are blocked, and
  `user:pass@` is stripped from the initial URL. Use `--allow-private` (or
  `RendererConfig::allow_private_networks`) only for local testing.
- **Servo's own connections go through a checking proxy.** Servo sends every
  http and https connection through a proxy that this crate runs on
  `127.0.0.1`. It checks each destination when it connects, and connects only
  to the addresses it checked, so a second DNS answer cannot move a connection
  to a private address (DNS rebinding). Redirects are followed by Servo, and
  every hop goes through the proxy. Ports on the Fetch standard's bad-port
  list are refused. The proxy takes its check from `--allow-private`, and its
  own address is never a destination.
- **Same-origin script requests are fetched here.** A same-origin GET from a
  page script (XHR or fetch) is fetched by this crate, so a synchronous XHR to
  a server that never answers gets a timeout. It is sent the page's cookies,
  follows only same-origin redirects (301, 302, 303, 307 and 308 with a
  `Location`), and is limited to 10 redirects. A proxied same-origin script
  GET whose redirect leaves its origin fails with a network error, unlike a
  browser, because the proxy keeps the 8-second synchronous XHR deadline; see
  [issue #19](https://github.com/kerryhatcher/CherryPi-Fetch/issues/19). A
  script fetch that starts out cross-origin and redirects is left to Servo,
  which applies CORS. Pages that send `Referrer-Policy: no-referrer` get no
  such timeout: their script requests are not recognised as same-origin, so
  they are left to Servo.
- **URL userinfo (`user:pass@`) is handled by entry point.** The CLI and
  `render_page` strip it from the input URL before Servo sees it. A load
  whose URL still carries it (for example a subresource the page names) is
  refused. The proxied script-GET chain strips it from each redirect target.
  For redirects Servo follows itself, the interceptor sees only the original
  URL, so this check does not cover their targets. A top-level redirect to a
  URL with userinfo can fail to finish within the render budget.
- **The WebSocket connection is the one place DNS rebinding remains.** Servo
  connects a WebSocket itself, without the proxy. The handshake is checked
  before it is sent, but the connection is resolved again, so a server that
  changes its answer between the two can point the socket at a private address.
  Refusing WebSockets in default mode is not done, because pages that use them
  would then fail.
- **HTTP proxies in the environment are ignored.** `HTTP_PROXY`,
  `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` do not change where requests go.
  Requests go to the checking proxy, which connects to the destination
  directly, so an upstream proxy is not supported.
- **Dedicated worker fetches are covered; service workers are untested.** A
  `data:` page can start a dedicated worker (there is a test), but it cannot
  register a service worker, so that path is unverified.
- **Not for CAPTCHA or login-walled pages.** A page that blocks automated
  browsers, or that needs a login, comes back blank or incomplete.
- **Prebuilt archives are only published for x86_64 Linux (glibc 2.39+) and
  Apple-silicon macOS.** Every other target builds from source.

## Licensing

This project's own code is [MIT licensed](LICENSE). The release binaries link
[Servo](https://servo.org) and other MPL-2.0 dependencies, so they include
MPL-2.0 code. Each MPL-2.0 dependency's source is on crates.io at the version
pinned in `Cargo.lock`; [`servo`](https://crates.io/crates/servo/0.8.0) itself
is 0.8.0.

Each release archive also contains a generated `THIRD_PARTY_LICENSES` file
that lists every dependency and its license text.

## Differences from konippi/servo-fetch

[konippi/servo-fetch](https://github.com/konippi/servo-fetch) is the existing
project that owns the `servo-fetch` name on crates.io. It is a much larger
tool: it renders to Markdown, JSON or PNG screenshots, crawls sites, maps
URLs from sitemaps, and ships a Rust library, Python and Node.js SDKs, an MCP
server and a Docker image. Its README says it blocks private and reserved IP
ranges and strips credentials from URLs. `cherrypi-fetch` blocks private
addresses by default and strips credentials from input URLs, within the
limits listed under Limitations.

`cherrypi-fetch` is deliberately smaller and focused on one job: return a
page's title and visible text to a caller that may be an agent. Its behaviour
is built around pages that misbehave:

- **A timeout on the page's own script requests.** The same-origin GET
  requests a page's scripts make (XHR and `fetch`) are proxied through
  `reqwest` with an 8-second timeout. konippi's README does not describe
  this. Other requests are left to Servo. A render that still stalls ends at
  its timeout: if it captured some text it is returned as incomplete; if it
  captured no text, it fails (`ok:false`, exit 1).
- **A clean state for every render.** Cookies, local and session storage and
  the HTTP cache are cleared before each page, so a site that blocked one
  visit does not poison the next. konippi's MCP section says its page tools
  run in fresh workers with no cookies or storage carried between calls;
  cherrypi-fetch also clears the HTTP cache before each render.
- **A Windows Firefox User-Agent by default.** konippi exposes a user agent
  on its crawl options, but its README does not say what its default is.

It returns text (and a JSON record with `--json`), not Markdown, a screenshot
or a schema, and it does not crawl.

The non-default `__test-hooks` feature turns the private-address check off for
the host and port pairs a test names. This crate's own tests enable it through
a dev-dependency, so it is not on for downstream users. Do not enable it in a
published build.
