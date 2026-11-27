# Changelog

<!-- Releasing: set the date on the version heading before tagging, e.g.
     `## [0.1.0] - 2026-01-31`; the release workflow rejects a stable tag
     without a dated heading. -->

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking

- `RendererConfig` gained an `allow_private_networks` field, so a struct
  literal needs updating; `RendererConfig::default()` is unchanged.

### Changed

- Servo's own connections go through a checking proxy. Each connection is
  checked when it is made, and connects only to the address it checked, so
  DNS rebinding no longer moves a connection to a private address. Servo
  follows redirects itself, and every hop goes through the proxy. Proxy
  environment variables are ignored; they used to make the render fail.
- Redirect statuses: only 301, 302, 303, 307 and 308 with a `Location` are
  redirects. Other responses, such as a 300 or a 304, reach the page as they
  are.
- Requests with `user:pass@` in their URL are refused before Servo fetches
  them.
- The non-default `test-hooks` feature is now `__test-hooks`.

### Added

- Private, loopback and link-local destinations are refused by default, for
  the page and every resource it loads, when they connect. Only `http` and
  `https` are fetched (`data`, `blob` and `about` are allowed locally; other
  schemes, including `file:`, are refused). Ports on the Fetch standard's
  bad-port list are refused. `user:pass@` is stripped from URLs.
  `--allow-private` and `RendererConfig::allow_private_networks` opt out for
  local testing.
- `strip_userinfo` is public, so a caller can strip credentials the same way
  the CLI does.
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

[Unreleased]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/kerryhatcher/CherryPi-Fetch/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/kerryhatcher/CherryPi-Fetch/releases/tag/v0.1.0
