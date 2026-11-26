# cherrypi-fetch

A tool that humans and agents can use to fetch data from most public pages,
even SPAs. It renders JavaScript-heavy pages in [Servo](https://servo.org) and
returns their title and visible text, as a library or a command-line tool. No
Chrome, Chromium or V8 is involved. Originally for the CherryPi AI agent
harness.

## Install

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install cherrypi-fetch    # build from source
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

## Library

```rust
// Cargo.toml: cherrypi-fetch = "0.1", url = "2"
async fn demo() -> Result<(), cherrypi_fetch::RenderError> {
    cherrypi_fetch::configure(cherrypi_fetch::RendererConfig {
        user_agent: "MyTool/1.0".into(),
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
  `user:pass@` is stripped from URLs. Use `--allow-private` (or
  `RendererConfig::allow_private_networks`) only for local testing.
- **Redirects are re-fetched, not followed by Servo.** In default mode a
  redirect hop that Servo would follow is re-requested by this crate, with
  every hop and target checked, and the final response is handed back to the
  render, so a redirect to a private address is refused. The first hop is
  therefore requested twice, the request's existing cookies are sent on every
  hop, cookies set by an intermediate redirect are discarded, and compressed
  bodies are decoded. A cross-origin script fetch (fetch/XHR) that redirects
  is refused outright, because the CORS check cannot be applied to an
  intercepted hop.
- **DNS rebinding is only partly closed.** Requests this crate makes itself
  are pinned to the addresses that were checked. The ones Servo makes are
  resolved again when it connects, so a server that changes its answer
  between the check and the connect can swap a public address for a private
  one.
- **HTTP proxies are refused.** If `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY`
  is set, the render fails unless private networks are allowed, because Servo
  would send the request through the proxy instead of the checked path.
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
server and a Docker image. Its README also describes safety work this project
does not do: it blocks private and reserved IP ranges and strips credentials
from URLs. The `cherrypi-fetch` CLI refuses URLs that are not http or https;
the library does not check the scheme.

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

The non-default `test-hooks` feature (used only by this crate's own
integration tests) turns the private-address check off for host and port pairs a test names.
Do not enable it in a published build.
