/**
 * cherrypi-fetch for Pi.
 *
 * Registers `fetch_page`, which shells out to the cherrypi-fetch CLI and
 * returns its `--json` record. The render itself happens in the CLI, so the
 * binary must be installed; Pi does not bundle Servo.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const DEFAULT_TIMEOUT_SECS = 20;
const MAX_TIMEOUT_SECS = 600;
// Match the MCP tool: default 40,000 characters, clamp at 200,000.
const DEFAULT_MAX_CHARS = 40_000;
const MAX_CHARS_LIMIT = 200_000;
const MAX_TITLE_CHARS = 500;

/** Extra time for the CLI to print its record after its own render timeout. */
const EXIT_GRACE_MS = 10_000;

const INSTALL_HINT = `Install cherrypi-fetch, then try again:
  cargo binstall cherrypi-fetch   # prebuilt binary, no Servo build
  cargo install --locked cherrypi-fetch    # build from source
Prebuilt binaries exist for x86_64 Linux and Apple-silicon macOS. See
https://github.com/kerryhatcher/CherryPi-Fetch#install`;

/** The object the CLI prints with `--json`. */
type FetchRecord = {
  url: string;
  ok: boolean;
  title: string | null;
  text: string | null;
  incomplete: string | null;
  truncated: boolean;
  error: string | null;
  elapsed_ms: number;
};

const parameters = Type.Object({
  url: Type.String({
    description: "http or https URL to render",
  }),
  timeout_secs: Type.Optional(
    Type.Integer({
      description: "Seconds to let the page load and settle",
      minimum: 1,
      maximum: MAX_TIMEOUT_SECS,
      default: DEFAULT_TIMEOUT_SECS,
    }),
  ),
  max_chars: Type.Optional(
    Type.Integer({
      description:
        "Keep at most this many characters of the page text (default 40000; values above 200000 are clamped)",
      minimum: 1,
      default: DEFAULT_MAX_CHARS,
    }),
  ),
});

/** Reject anything the CLI would also reject, before spawning it. */
function checkUrl(raw: string): void {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new Error(`Not a valid URL: ${raw}`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(
      `Unsupported URL scheme ${url.protocol.replace(/:$/, "")}: use http or https.`,
    );
  }
}

function truncateChars(value: string, max: number): { text: string; truncated: boolean } {
  const chars = Array.from(value);
  if (chars.length <= max) {
    return { text: value, truncated: false };
  }
  return { text: chars.slice(0, max).join(""), truncated: true };
}

function parseRecord(stdout: string): FetchRecord | undefined {
  const line = stdout
    .split("\n")
    .map((value) => value.trim())
    .filter((value) => value.length > 0)
    .pop();
  if (!line) {
    return undefined;
  }
  try {
    return JSON.parse(line) as FetchRecord;
  } catch {
    return undefined;
  }
}

function isMissingBinary(stderr: string): boolean {
  return /ENOENT|not found|no such file|is not recognized/i.test(stderr);
}

function formatRecord(record: FetchRecord, maxChars: number): string {
  const title = truncateChars(record.title ?? "(none)", MAX_TITLE_CHARS);
  const lines = [`URL: ${record.url}`, `Title: ${title.text}`];

  if (title.truncated) {
    lines.push(`Note: title truncated at ${MAX_TITLE_CHARS} characters.`);
  }
  if (record.error) {
    lines.push(`Error: ${record.error}`);
  }
  if (record.incomplete) {
    lines.push(
      `Incomplete: ${record.incomplete}. The text below is the last snapshot; do not re-fetch in a loop.`,
    );
  }
  if (record.truncated) {
    lines.push(`Note: text truncated at ${maxChars} characters (max_chars).`);
  }
  if (!record.ok || record.text === null) {
    return lines.join("\n");
  }

  lines.push("", record.text.length > 0 ? record.text : "(no visible text)");
  return lines.join("\n");
}

export default function cherrypiFetchExtension(pi: ExtensionAPI) {
  pi.registerTool({
    name: "fetch_page",
    label: "Fetch Page",
    description:
      "Render a JavaScript-heavy page in Servo and return its title and visible text. Use for SPAs, or when a plain fetch returns empty or only \"loading…\".",
    promptSnippet:
      "Render a JavaScript-heavy page with cherrypi-fetch and return its title and visible text",
    promptGuidelines: [
      "Use fetch_page when a plain fetch is empty or a page is JavaScript-rendered; do not use it for CAPTCHAs, paywalls or login-walled pages.",
    ],
    parameters,

    async execute(_toolCallId, params, signal) {
      checkUrl(params.url);

      const timeoutSecs = params.timeout_secs ?? DEFAULT_TIMEOUT_SECS;
      const requestedMaxChars = params.max_chars ?? DEFAULT_MAX_CHARS;
      const maxChars = Math.min(Math.max(requestedMaxChars, 1), MAX_CHARS_LIMIT);
      const args = [
        "--json",
        "--timeout",
        String(timeoutSecs),
        "--max-chars",
        String(maxChars),
        // Keep a URL that starts with a dash from becoming a CLI option.
        "--",
        params.url,
      ];

      const result = await pi.exec("cherrypi-fetch", args, {
        signal,
        // Let the CLI hit its own render timeout and still print an
        // `incomplete` record before Pi kills it.
        timeout: timeoutSecs * 1000 + EXIT_GRACE_MS,
        // JSON escaping can make the output larger than maxChars; a long
        // title still counts against this cap.
        maxBuffer: Math.max(maxChars * 8 + 65_536, 1_048_576),
      });

      // `killed` is also true when maxBuffer is exceeded, so check this before
      // reporting a timeout.
      if (result.truncated) {
        throw new Error(
          "cherrypi-fetch returned more output than Pi can buffer, so the record was cut off. Lower max_chars or run cherrypi-fetch directly.",
        );
      }

      const record = parseRecord(result.stdout);

      if (!record) {
        if (result.code === 1 && result.stdout.trim().length === 0 && isMissingBinary(result.stderr)) {
          throw new Error(`cherrypi-fetch was not found on PATH.\n\n${INSTALL_HINT}`);
        }
        if (result.killed) {
          if (signal?.aborted) {
            throw new Error("cherrypi-fetch was cancelled.");
          }
          throw new Error(
            `cherrypi-fetch did not finish within ${timeoutSecs + EXIT_GRACE_MS / 1000}s and was stopped.`,
          );
        }
        const detail =
          result.stderr.trim() || result.stdout.trim() || `exit code ${result.code}`;
        throw new Error(`cherrypi-fetch did not return a JSON record: ${detail}`);
      }

      const text = formatRecord(record, maxChars);

      // A failed render is still a well-formed record; keep it in `details`
      // so a caller can inspect the error, and mark the tool result as failed.
      if (!record.ok) {
        return {
          content: [{ type: "text" as const, text }],
          details: record,
          isError: true,
        };
      }

      return {
        content: [{ type: "text" as const, text }],
        details: record,
      };
    },
  });
}
