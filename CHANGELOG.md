# Changelog

<!-- Releasing: set the date on the version heading before tagging, e.g.
     `## [0.1.0] - 2026-01-31`; the release workflow rejects a stable tag
     without a dated heading. -->

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `cherrypi-fetch mcp`, a stdio MCP server with one tool, `fetch_page`. It
  renders a page in Servo and returns its title and text. Invalid input and a
  failed render are tool errors (`isError`). The server is behind the `mcp`
  feature, which is on by default. `--no-default-features --features cli`
  builds the command without it.

### Changed

- `rmcp` is a caret requirement (`3.12`), so library users are not pinned to
  one patch release. Install with `cargo install --locked` to build with the
  versions in `Cargo.lock`.

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

[0.1.1]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kerryhatcher/CherryPi-Fetch/releases/tag/v0.1.0
