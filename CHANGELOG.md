# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - unreleased

Initial release: the `cherrypi-fetch` library and CLI, moved out of CherryPi's
`crates/servo-fetch` into their own crate.

### Added

- `cherrypi-fetch` as a library and as a `cherrypi-fetch` command. The API is
  `render_page`, with a process-wide `configure` for the User-Agent and a
  `shutdown_renderer` for a clean exit.
- The renderer behaviour that lived in CherryPi:
  - same-origin synchronous XHR and `fetch` requests are proxied through
    `reqwest` with an 8-second timeout, so a page waiting on a request that
    never answers cannot hang the render;
  - cookies, local and session storage and the HTTP cache are cleared before
    every render, so one bot block cannot poison the next;
  - a configurable User-Agent, defaulting to Windows Firefox, because some
    sites never answer a desktop Linux user agent;
  - snapshots that are discarded when a navigation starts, so text from an
    earlier document does not leak through;
  - one Servo instance per process, on its own thread, ended by
    `shutdown_renderer`;
  - timeouts bounded to 1–600 seconds and text that is truncated rather than
    unbounded.
- CLI `--json` records and exit codes: 0 when every page rendered fully, 2
  when at least one was incomplete, 1 when at least one failed or the
  arguments were bad.

[0.1.0]: https://github.com/kerryhatcher/CherryPi-Fetch/releases/tag/v0.1.0
