#!/usr/bin/env bash
# Publishes dist-release/<version>/ as GitHub Release v<version>, with the
# updater feed (latest.json) that installed apps check:
#
#   scripts/release/publish.sh <notes-file>
#
# Fallback for a macOS build made on a Mac (build-macos.sh); the normal
# route is scripts/release/tag.sh, which leaves every build to GitHub
# Actions. Pushing the tag starts that workflow, which then rebuilds all
# platforms and replaces this release's files and latest.json. Publishing is an outward action: agents
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
