---
name: cherrypi-fetch
description: Render JavaScript-heavy pages with Servo and return their visible text. Use when a plain fetch is empty, shows only "loading…", or the page is an SPA that needs its JavaScript to run. Not for CAPTCHAs, paywalls or login-walled pages.
license: MIT
---

# cherrypi-fetch

`cherrypi-fetch` renders a page in Servo, runs its JavaScript, waits for the page
to settle, and returns the title and visible text. It is not a plain HTTP
fetcher: it is for pages a plain fetch cannot read.

## When to use it

Use it when:

- The page is JavaScript-rendered: an SPA, or a page whose plain fetch returns
  "loading…".
- A plain fetch came back empty, or contained only a script shell.

Do not use it to get past a CAPTCHA, a paywall or a login.

## How to use it

1. Prefer the `fetch_page` tool when it is available: the MCP server and the
   Pi package both expose it. It takes `url`, optional `timeout_secs` (1–600,
   default 20) and optional `max_chars` (default 40000; larger values are
   clamped to 200000).

   - **MCP:** the result is a text block plus `structuredContent` with `url`,
     `title`, `text`, `incomplete`, `truncated` and `elapsed_ms`. There is no
     `ok` or `error` field. A bad argument or a page that cannot be rendered is
     an `isError` result whose text is the reason.
   - **Pi:** the model-facing text is formatted, and the tool's `details` field
     is the CLI record described below, including `ok` and `error`.

2. Otherwise run the CLI and read the exit code:

```sh
cherrypi-fetch --json --max-chars 40000 -- https://example.com/
```

Put `--` before the URL so the URL cannot be read as an option.

A render failure or an unsupported URL scheme prints one JSON record per URL
to stdout and diagnostics to stderr. The record has `url`, `ok`, `title`,
`text`, `incomplete`, `truncated`, `error` and `elapsed_ms`. An argument error
prints nothing to stdout; the reason is on stderr.

Exit status:

- `0`: every page rendered fully.
- `2`: at least one page did not settle and none failed. The record still has
  text, plus an `incomplete` reason.
- `1`: at least one page failed, or the arguments were bad. A failed render or
  a rejected URL scheme still prints an `ok:false` record with `error`. An
  argument error prints no record; read stderr.

## How to read the result

- **MCP:** if the tool result is an error, report its text. Otherwise check
  `structuredContent`. If `incomplete` is set, the page did not settle; report
  that and the last text captured. Do not re-fetch in a loop.
- **CLI or Pi details:** if `ok` is false, report `error`. If `incomplete` is
  set, the page did not settle; report that and the last text captured. Do not
  re-fetch in a loop.
- If the text is blank, assume a bot block or a login wall. Say so. Do not
  hunt for mirrors or alternate sources unless the user asked for one.
- If `truncated` is true, the text was cut at `max_chars`. Raise the limit only
  when the missing tail matters.

## Install

If the binary is missing, install it:

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install cherrypi-fetch    # build from source
```

Prebuilt binaries exist for x86_64 Linux (glibc) and Apple-silicon macOS.
Other targets build Servo from source and need the system packages listed in
the project README.
