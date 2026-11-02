#!/usr/bin/env bash
# Builds the signed macOS release (universal .app, .dmg and the updater
# bundle) into dist-release/<version>/. Run on a Mac from any folder:
#
#   scripts/release/build-macos.sh
#
# Needs the updater signing key: TAURI_SIGNING_PRIVATE_KEY (the key or a
# path to it), default the file in JC_UPDATER_KEY or
# ~/.tauri/jenderalcode-updater.key. The key never goes into the repo.
set -euo pipefail
cd "$(dirname "$0")/../.."

KEY_FILE=${JC_UPDATER_KEY:-$HOME/.tauri/jenderalcode-updater.key}
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  [ -f "$KEY_FILE" ] || { echo "no updater key: set TAURI_SIGNING_PRIVATE_KEY or JC_UPDATER_KEY" >&2; exit 1; }
  export TAURI_SIGNING_PRIVATE_KEY="$KEY_FILE"
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist-release/$VERSION"
B=target/universal-apple-darwin/release/bundle
mkdir -p "$OUT"

npx --yes @tauri-apps/cli@2 build --target universal-apple-darwin --bundles app,dmg \
  --config '{"bundle":{"createUpdaterArtifacts":true}}'

cp "$B/dmg/JenderalCode_${VERSION}_universal.dmg" "$OUT"/
cp "$B/macos/JenderalCode.app.tar.gz" "$OUT/JenderalCode_${VERSION}_universal.app.tar.gz"
cp "$B/macos/JenderalCode.app.tar.gz.sig" "$OUT/JenderalCode_${VERSION}_universal.app.tar.gz.sig"
hdiutil verify "$OUT/JenderalCode_${VERSION}_universal.dmg" >/dev/null
lipo -archs "$B/macos/JenderalCode.app/Contents/MacOS/jenderalcode-desktop"
(cd "$OUT" && rm -f SHA256SUMS && shasum -a 256 -- *.dmg *.tar.gz *.AppImage *.deb *.rpm *.exe 2>/dev/null > SHA256SUMS || true)
ls -lh "$OUT"
