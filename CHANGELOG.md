# Changelog

<!-- Releasing: set the date on the version heading before tagging, e.g.
     `## [0.1.0] - 2026-01-31`; the release workflow rejects a stable tag
     without a dated heading. -->

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-27

### Breaking

- Private, loopback and link-local destinations are refused by default, for
  the page and every resource it loads, when they connect (a WebSocket is
  resolved again after its check; see the README). `localhost` and intranet
  pages that rendered with 0.1.x now fail. `--allow-private` (CLI and
  `cherrypi-fetch mcp`) and `RendererConfig::allow_private_networks` opt out,
  for local testing.
- Only `http` and `https` are fetched (`data`, `blob` and `about` are allowed
  locally; other schemes, including `file:`, are refused). Ports on the Fetch
  standard's bad-port list are refused.
- `RendererConfig` gained an `allow_private_networks` field, so a struct
  literal needs updating; `RendererConfig::default()` is unchanged.

### Added

- `cherrypi-fetch mcp`, a stdio MCP server with one tool, `fetch_page`. It
  renders a page in Servo and returns its title and text. Invalid input and a
  failed render are tool errors (`isError`). The server is behind the `mcp`
  feature, which is on by default. `--no-default-features --features cli`
  builds the command without it.
- Agent plugins for Claude Code and Codex (an MCP server entry plus a skill),
  and a Pi package (a `fetch_page` extension plus the skill). They install from
  this repository's marketplaces; see "Use it from your agent" in the README.
- `strip_userinfo` is public, so a caller can strip credentials the same way
  the CLI does.

### Changed

- Servo's own connections go through a checking proxy. Each connection is
  checked when it is made, and connects only to the address it checked, so
  DNS rebinding no longer moves a connection to a private address. Servo
  follows redirects itself, and every hop goes through the proxy. Proxy
  environment variables are ignored; they used to make the render fail.
- URL userinfo (`user:pass@`) is stripped from the input URL by the CLI,
  `render_page` and `fetch_page`, and never appears in their output. A load
  whose URL still carries it, such as a subresource the page names, is
  refused. Redirect targets that Servo follows itself are not covered; see the
  README.
- Same-origin script requests, which this crate fetches itself, decode gzip,
  brotli, deflate and zstd bodies. Their redirects send each hop the page's
  cookies for that URL and store each hop's `Set-Cookie`. A redirect that
  leaves the origin is a network error (#19).
- Redirect statuses: only 301, 302, 303, 307 and 308 with a `Location` are
  redirects. Other responses, such as a 300 or a 304, reach the page as they
  are.
- `fetch_page` clamps any `max_chars` above 200000, however large.
- `rmcp` is a caret requirement (`3.12`), so library users are not pinned to
  one patch release. Install with `cargo install --locked` to build with the
  versions in `Cargo.lock`.
- The non-default `test-hooks` feature is now `__test-hooks`.

### Fixed

- A page whose main document fails to load (connection refused, DNS failure,
  a bad port, or a TLS error) is a failed render: `RenderError::Failed` with
  Servo's reason, `ok:false` and exit 1 from the CLI, and `isError` from MCP.
  It used to come back as `ok:true` with Servo's error page as the text.
- `cherrypi-fetch mcp` exits on SIGTERM or SIGINT while its client keeps stdin
  open. It waits at most two seconds for the service to stop, then stops
  Servo. A render already in progress still finishes first, and the server
  logs that to stderr.
- A call that is cancelled while it waits for the renderer no longer renders
  the page once its turn comes. A render that has started still runs to its
  end, because Servo cannot abort a load.
- `cherrypi-fetch --json mcp` (an option before `mcp`) explains that `mcp` is a
  subcommand, rather than reporting an invalid URL.
- The `fetch_page` input schema states the ranges and defaults, and whole
  numbers sent as floats (`20.0`) are accepted.

## [0.1.1] - 2026-11-26

### Fixed

- The proxy's script requests follow Servo and trust the system root store
  (`rustls-tls-native-roots-no-provider`), and HTTP/2 is enabled. A page
  behind a corporate or custom CA no longer renders incomplete because its
  proxied requests fail TLS.

## [0.1.0] - 2026-11-26

Initial release: the `cherrypi-fetch` library and CLI, moved out of CherryPi's
`crates/servo-fetch` into their own crate.

### Added

- `cherrypi-fetch` as a library and as a `cherrypi-fetch` command. The API is
  `render_page`, with a process-wide `configure` for the User-Agent and a
  `shutdown_renderer` for a clean exit.
- The renderer behaviour that lived in CherryPi:
  - same-origin GET requests from scripts (XHR and `fetch`) are proxied
    through `reqwest` with an 8-second timeout; other requests are left to
    Servo. A render that still stalls ends at its timeout: it is returned as
    incomplete if it captured some text, or fails (`ok:false`, exit 1) if it
    captured none;
  - cookies, local and session storage and the HTTP cache are cleared before
    every render, so one bot block cannot poison the next;
  - a configurable User-Agent, defaulting to Windows Firefox, because some
    sites never answer a desktop Linux user agent;
  - snapshots that are discarded when a navigation starts, so text from an
    earlier document does not leak through;
  - one Servo instance per process, on its own thread, ended by
    `shutdown_renderer`;
  - timeouts bounded to 1–600 seconds; `--max-chars` truncates text on
    request.
- Page titles and text are stripped of C0 and C1 control characters other
  than newline and tab, and of the bidi overrides and isolates, so a page
  cannot inject terminal escapes or reorder a caller's output.
- CLI `--json` records and exit codes: 0 when every page rendered fully, 1
  when at least one failed (even if others were incomplete) or the arguments
  were bad, 2 when at least one was incomplete and none failed.

[Unreleased]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kerryhatcher/CherryPi-Fetch/releases/tag/v0.1.0
