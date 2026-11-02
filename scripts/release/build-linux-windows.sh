#!/usr/bin/env bash
# Builds the Linux bundles and the Windows installer inside Docker.
# Run from the repository root on a Linux x86_64 host with Docker:
#
#   scripts/release/build-linux-windows.sh [linux|windows|all]
#
# Output goes to dist-release/<version>/. CPU and memory are capped
# (JC_CPUS, JC_MEMORY) so the build does not starve other services on a
# shared host. Updater bundles (.AppImage / -setup.exe plus .sig) are
# signed with TAURI_SIGNING_PRIVATE_KEY (the key itself, not a path: the
# container cannot see host files) from the environment.
set -euo pipefail

TARGET=${1:-all}
: "${TAURI_SIGNING_PRIVATE_KEY:?set TAURI_SIGNING_PRIVATE_KEY to the updater key (see docs/RELEASE.md)}"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}
UPDATER='{"bundle":{"createUpdaterArtifacts":true}}'
CPUS=${JC_CPUS:-4}
MEMORY=${JC_MEMORY:-5g}
IMAGE=jenderalcode-release-builder

cd "$(dirname "$0")/../.."
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist-release/$VERSION"
mkdir -p "$OUT"

docker build -t "$IMAGE" scripts/release

run() {
  docker run --rm --cpus "$CPUS" --memory "$MEMORY" \
    -e CARGO_BUILD_JOBS="$CPUS" \
    -e TAURI_SIGNING_PRIVATE_KEY -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
    -u "$(id -u):$(id -g)" -e HOME=/tmp/home \
    -e CARGO_HOME=/cache/cargo -e XWIN_CACHE_DIR=/cache/xwin -e npm_config_cache=/cache/npm \
    -v "$PWD":/src \
    -v jenderalcode-cache:/cache \
    "$IMAGE" bash -c "mkdir -p /tmp/home && cd /src && $1"
}

# Named volumes are created root-owned; hand the cache to the build user.
docker run --rm -v jenderalcode-cache:/cache "$IMAGE" chown -R "$(id -u):$(id -g)" /cache

run "npm --prefix frontend ci"

if [ "$TARGET" = linux ] || [ "$TARGET" = all ]; then
  run "npx --yes @tauri-apps/cli@2 build --bundles deb,rpm,appimage --config '$UPDATER'"
  cp target/release/bundle/deb/*.deb target/release/bundle/rpm/*.rpm \
     target/release/bundle/appimage/*.AppImage target/release/bundle/appimage/*.AppImage.sig "$OUT"/
fi

if [ "$TARGET" = windows ] || [ "$TARGET" = all ]; then
  run "npx --yes @tauri-apps/cli@2 build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis --config '$UPDATER'"
  cp target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe.sig "$OUT"/
fi

(cd "$OUT" && rm -f SHA256SUMS && sha256sum -- $(ls | grep -vE '\.sig$|^latest\.json$') > SHA256SUMS)
ls -lh "$OUT"
