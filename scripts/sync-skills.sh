#!/usr/bin/env bash
# Keep every plugin's copy of the shared skills identical to skills/.
#
# skills/<name>/SKILL.md is the source of truth. Each plugin ships a copy so
# that it stays installable as a standalone directory. CI runs this script and
# fails when it changes the working tree, so edit skills/ and re-run it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_dir="$root/skills"

targets=(
  plugins/claude-code/skills
  plugins/codex/skills
  plugins/pi/skills
)

if [ ! -d "$source_dir" ]; then
  echo "error: $source_dir does not exist" >&2
  exit 1
fi

for target in "${targets[@]}"; do
  destination="$root/$target"
  mkdir -p "$destination"

  # Remove copies of skills that no longer exist in skills/.
  for existing in "$destination"/*/; do
    [ -d "$existing" ] || continue
    name="$(basename "$existing")"
    if [ ! -d "$source_dir/$name" ]; then
      echo "removing stale copy $target/$name"
      rm -rf "$existing"
    fi
  done

  # Copy each shared skill. Only SKILL.md is shared today; supporting files
  # would need an explicit copy rule here.
  for skill in "$source_dir"/*/; do
    [ -d "$skill" ] || continue
    name="$(basename "$skill")"
    if [ ! -f "$skill/SKILL.md" ]; then
      echo "error: $skill/SKILL.md is missing" >&2
      exit 1
    fi
    # Remove the whole destination skill directory so a file that is no
    # longer in the source cannot survive a sync.
    rm -rf "$destination/$name"
    mkdir -p "$destination/$name"
    cp "$skill/SKILL.md" "$destination/$name/SKILL.md"
  done

  echo "synced $target"
done
