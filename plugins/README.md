# Agent plugins

These directories package the same `cherrypi-fetch` skill for three agent
hosts. Each one calls the `cherrypi-fetch` binary that must already be on
`PATH`:

```sh
cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
cargo install --locked cherrypi-fetch    # build from source
```

The skill lives once at [`../skills/cherrypi-fetch/SKILL.md`](../skills/cherrypi-fetch/SKILL.md).
Run [`../scripts/sync-skills.sh`](../scripts/sync-skills.sh) after editing it;
CI fails when a plugin copy is out of date.

## Claude Code

The repository root is a Claude Code marketplace (`cherrypi-fetch`) whose
plugin source is `./plugins/claude-code`.

```text
/plugin marketplace add kerryhatcher/CherryPi-Fetch
/plugin install cherrypi-fetch@cherrypi-fetch
```

For a local checkout, run `/plugin marketplace add /absolute/path/to/CherryPi-Fetch`
instead. The plugin starts the MCP server with `cherrypi-fetch mcp` and exposes
the `fetch_page` tool.

Validate a change without starting a session:

```sh
claude plugin validate --strict ./plugins/claude-code
claude plugin validate --strict ./.claude-plugin/marketplace.json
```

## Codex

The repository root is a Codex repo marketplace (`.agents/plugins/marketplace.json`)
whose plugin lives in `./plugins/codex`. It uses the supported
`.codex-plugin/plugin.json` compatibility layout and `./.mcp.json`; the
portable `plugin.json` + `mcp.json` layout is not used.

```sh
codex plugin marketplace add kerryhatcher/CherryPi-Fetch
codex plugin add cherrypi-fetch@cherrypi-fetch
```

For a local checkout, run `codex plugin marketplace add .` from the repository
root. The plugin starts the MCP server with `cherrypi-fetch mcp` and exposes
the `fetch_page` tool.

## Pi

The Pi package ships a `fetch_page` tool that runs
`cherrypi-fetch --json --max-chars N URL` directly, plus the shared skill. Pi
1.6 has no git-source subpath syntax, so the repository root `package.json` is
the installable package and points into `plugins/pi/`:

```sh
pi install git:github.com/kerryhatcher/CherryPi-Fetch
```

From a checkout, install only the Pi plugin directory:

```sh
pi install ./plugins/pi -l    # project-local; or without -l for a personal install
pi -e ./plugins/pi            # one-off test without adding it to settings
```

`pi list` shows the installed package. Pi loads the `.ts` extension through
jiti, so there is no build step.

Alternatively, register the same MCP server without the package:

```sh
pi mcp add cherrypi-fetch -- cherrypi-fetch mcp
```

## Shared requirements

- **Claude Code and Codex:** `cherrypi-fetch` 0.2.0 or newer on `PATH`, because
  both plugins launch their MCP server with `cherrypi-fetch mcp`.
- **Pi:** `cherrypi-fetch` 0.1.x or newer on `PATH`. The extension only needs
  the CLI's `--json`, `--timeout` and `--max-chars` options, which 0.1.x has.
  The Pi skill also works when `fetch_page` comes from the MCP server instead.

## Versions

Every manifest is `0.2.0`, the first release with the `mcp` subcommand.
`scripts/check-plugin-versions.sh` requires the manifests to agree with each
other, and to equal `Cargo.toml` once the crate reaches their version. Before
that (for example on this branch, where Cargo is 0.1.1) the manifests may be
ahead, but never behind.

## Sources checked (2026-11-26)

- **Claude Code 2.1.332:** `claude plugin --help` and
  `claude plugin validate --help`; both manifests pass
  `claude plugin validate --strict`.
- **Codex CLI 0.172.0:** <https://developers.openai.com/codex/plugins/build>
  (compatibility `.codex-plugin/plugin.json` layout and
  `.agents/plugins/marketplace.json`), plus `codex plugin --help`,
  `codex plugin marketplace add --help` and `codex plugin add --help`. A
  scratch `CODEX_HOME` installed the plugin with
  `codex plugin add cherrypi-fetch@cherrypi-fetch`.
- **Pi 1.6.1:** `docs/packages.md`, `docs/extensions.md` and `docs/cli.md` in
  the installed package, plus `pi install --help` and `pi mcp add --help`.
  `pi install ./plugins/pi -l` and `pi install . -l` both succeeded in a
  scratch directory.
