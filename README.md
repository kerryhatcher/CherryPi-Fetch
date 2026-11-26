# cherrypi-fetch

A tool that humans and agents can use to fetch data from any webpage, even
SPAs. It renders JavaScript-heavy pages in [Servo](https://servo.org) and
returns their title and visible text, as a library or a command-line tool. No
Chrome, Chromium or V8 is involved. Originally for the CherryPi AI agent
harness.

## Install

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install cherrypi-fetch    # build from source
```

`cargo binstall` needs [cargo-binstall](https://github.com/cargo-bins/cargo-binstall)
and fetches the prebuilt archive from the GitHub release. The first
`cargo install` builds Servo, so it needs the
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

URLs are rendered one after another. Each one starts with no cookies, storage
or cache left by the ones before it.

Page data goes to stdout and diagnostics to stderr (set `RUST_LOG` for more).
Each `--json` record looks like this:

```json
{"url":"…","ok":true,"title":"…","text":"…","incomplete":null,"truncated":false,"error":null,"elapsed_ms":9784}
```

`incomplete` says why when a page did not settle in time; its text is then the
last snapshot taken. `error` is set when `ok` is false.

**Exit status:** 0 when every page rendered fully, 2 when at least one was
incomplete, 1 when at least one failed or the arguments were bad.

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
sudo apt-get install -y \
    build-essential cmake pkg-config clang libclang-dev \
    libfontconfig1-dev libegl1-mesa-dev libegl1 libgl1-mesa-dri \
    fonts-dejavu-core
```

On macOS, the Xcode Command Line Tools are enough; CI installs nothing else.

With those installed, `cargo fmt`, `cargo clippy`, `cargo test` and
`cargo build --no-default-features` work (the same checks CI runs). The
render tests need a GL implementation; on a headless machine set
`LIBGL_ALWAYS_SOFTWARE=1` so Mesa uses its software rasterizer.

## Why the default User-Agent claims Windows

Some sites never answer requests from desktop Linux user agents. Simbli
(eboardsolutions.com) is one: a page there waits forever on a synchronous
request. To keep such a page from hanging the render, the page's same-origin
script requests are made with an 8-second timeout.

## Licensing

This project's own code is [MIT licensed](LICENSE). It links the
[Servo](https://servo.org) browser engine, which is licensed under the
[Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/), so the
release binaries include MPL-2.0 code. Servo's source is available at
<https://github.com/servo/servo>.

## Differences from konippi/servo-fetch

[konippi/servo-fetch](https://github.com/konippi/servo-fetch) is the existing
project that owns the `servo-fetch` name on crates.io. It is a much larger
tool: it renders to Markdown, JSON or PNG screenshots, crawls sites, maps
URLs from sitemaps, and ships a Rust library, Python and Node.js SDKs, an MCP
server and a Docker image.

`cherrypi-fetch` is deliberately smaller and focused on one job: return a
page's title and visible text to a caller that may be an agent. Its behaviour
is built around pages that misbehave:

- **A timeout on the page's own synchronous requests.** Same-origin script
  requests are proxied through `reqwest` with an 8-second timeout, so a page
  that waits on a request that never answers cannot hang the render.
- **A clean state for every render.** Cookies, local and session storage and
  the HTTP cache are cleared before each page, so a site that blocked one
  visit does not poison the next.
- **A configurable User-Agent**, with a Windows Firefox default, because some
  sites never answer a desktop Linux user agent.

It returns text (and a JSON record with `--json`), not Markdown, a screenshot
or a schema, and it does not crawl.
