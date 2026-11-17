#!/usr/bin/env bash
# Starts a release: tags the committed version (after bump-version.sh) with
# an annotated tag whose message is the release notes, and pushes main and
# the tag. GitHub Actions (.github/workflows/release.yml) then builds macOS,
# Linux and Windows and publishes the release with latest.json; nothing is
# built on this machine.
#
#   scripts/release/tag.sh <notes-file>
#
# Publishing is an outward action: agents ask the maintainer first.
set -euo pipefail
NOTES=${1:?usage: tag.sh <notes-file>}
cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"

[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "commit your changes first" >&2; exit 1; }
[ "$(node -p 'require("./apps/desktop/tauri.conf.json").version')" = "$VERSION" ] ||
  { echo "run scripts/release/bump-version.sh first" >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  [ "$(git rev-parse "$TAG^{commit}")" = "$(git rev-parse HEAD)" ] || { echo "$TAG already tags another commit" >&2; exit 1; }
else
  git tag -a "$TAG" -F "$NOTES"
fi
git push origin HEAD
git push origin "$TAG"
echo "pushed $TAG; follow the build with: gh run watch \$(gh run list -w Release -L 1 --json databaseId -q '.[0].databaseId')"
