#!/usr/bin/env bash
# Check that every distributed manifest carries the crate's version.
#
# A pre-release branch can have manifests ahead of Cargo.toml: for example
# Cargo 0.1.1 with 0.2.0 manifests that wait for the release. The manifests
# must always agree with each other. Once Cargo.toml reaches a manifest
# version, every manifest must equal it, and it may never be behind.
#
# Usage: scripts/check-plugin-versions.sh [repo-root]
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cargo_toml="$root/Cargo.toml"

cargo_version="$(
  sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$cargo_toml" | head -n 1
)"
if [ -z "$cargo_version" ]; then
  echo "error: could not read the [package] version from $cargo_toml" >&2
  exit 1
fi

python3 - "$root" "$cargo_version" <<'PY'
import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
cargo_version = sys.argv[2]


def key(version: str):
    """Order numeric versions; fall back to the raw string on a suffix."""
    parts = []
    for piece in version.split("."):
        match = re.match(r"^(\d+)(.*)$", piece)
        if not match:
            return (1, 0, version)
        parts.append((int(match.group(1)), match.group(2)))
    return (0, tuple(parts))


def compare(version: str):
    first, rest = key(version)
    return (first, rest)


versions = {}
for rel in (
    "package.json",
    "plugins/pi/package.json",
    "plugins/claude-code/.claude-plugin/plugin.json",
    "plugins/codex/.codex-plugin/plugin.json",
):
    path = root / rel
    if not path.is_file():
        sys.exit(f"error: missing {rel}")
    versions[rel] = json.loads(path.read_text())["version"]

marketplace = root / ".claude-plugin/marketplace.json"
entry = json.loads(marketplace.read_text())["plugins"][0]
versions[".claude-plugin/marketplace.json plugins[0]"] = entry["version"]

if len(set(versions.values())) != 1:
    for rel, version in versions.items():
        print(f"  {rel}: {version}")
    sys.exit("error: the plugin manifests disagree with each other")

manifest_version = next(iter(versions.values()))

if cargo_version == manifest_version:
    print(f"versions match: {cargo_version}")
elif compare(cargo_version) > compare(manifest_version):
    sys.exit(
        f"error: Cargo.toml is {cargo_version} but the plugin manifests are "
        f"{manifest_version}; update every manifest for this release"
    )
else:
    print(
        f"ok: Cargo.toml is {cargo_version}, plugin manifests are "
        f"{manifest_version}; equality is required once Cargo reaches "
        f"{manifest_version}"
    )
PY
