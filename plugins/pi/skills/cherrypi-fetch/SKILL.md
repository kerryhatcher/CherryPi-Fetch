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

1. Prefer the `fetch_page` MCP tool when it is available. It takes `url` and
   optional `timeout_secs` and `max_chars`, and returns the same record as the
   CLI's `--json` output.
2. Otherwise run the CLI and read the exit code:

```sh
cherrypi-fetch --json --max-chars 20000 https://example.com/
```

The CLI writes one JSON object per URL to stdout and diagnostics to stderr.
The record has `url`, `ok`, `title`, `text`, `incomplete`, `truncated`,
`error` and `elapsed_ms`.

Exit status:

- `0`: every page rendered fully.
- `2`: at least one page did not settle and none failed. The record still has
  text, plus an `incomplete` reason.
- `1`: at least one page failed, or the arguments were bad. The record has
  `ok:false` and the reason in `error`.

## How to read the result

- If `incomplete` is set, the page did not settle in time. Report that and the
  last text captured. Do not re-fetch in a loop; a second run usually gives the
  same result.
- If `ok` is false, report `error` to the user.
- If the text is blank, assume a bot block or a login wall. Say so. Do not
  hunt for mirrors or alternate sources unless the user asked for one.
- If `truncated` is true, the text was cut at `--max-chars`. Raise the limit
  only when the missing tail matters.

## Install

If the binary is missing, install it:

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install cherrypi-fetch    # build from source
```

Prebuilt binaries exist for x86_64 Linux (glibc) and Apple-silicon macOS.
Other targets build Servo from source and need the system packages listed in
the project README.
