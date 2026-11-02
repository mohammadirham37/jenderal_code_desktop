#!/usr/bin/env bash
# Publishes dist-release/<version>/ as GitHub Release v<version>, with the
# updater feed (latest.json) that installed apps check:
#
#   scripts/release/publish.sh <notes-file>
#
# Run after build-macos.sh (and build-linux-windows.sh, with its output
# copied into the same folder). Publishing is an outward action: agents
# ask the maintainer first.
set -euo pipefail
NOTES=${1:?usage: publish.sh <notes-file>}
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist-release/$VERSION"

node scripts/release/latest-json.mjs "$VERSION" "$NOTES"
git rev-parse "v$VERSION" >/dev/null 2>&1 || git tag "v$VERSION"
git push origin "v$VERSION"
# .sig files are not uploaded: latest.json carries the signatures.
gh release create "v$VERSION" --title "v$VERSION" --notes-file "$NOTES" \
  $(find "$OUT" -maxdepth 1 -type f ! -name '*.sig' | sort)
