#!/usr/bin/env bash
# Builds the Linux bundles and the Windows installer inside Docker.
# Run from the repository root on a Linux x86_64 host with Docker:
#
#   scripts/release/build-linux-windows.sh [linux|windows|all]
#
# Output goes to dist-release/<version>/. CPU and memory are capped
# (JC_CPUS, JC_MEMORY) so the build does not starve other services on a
# shared host.
set -euo pipefail

TARGET=${1:-all}
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
  run "npx --yes @tauri-apps/cli@2 build --bundles deb,rpm,appimage"
  cp target/release/bundle/deb/*.deb target/release/bundle/rpm/*.rpm \
     target/release/bundle/appimage/*.AppImage "$OUT"/
fi

if [ "$TARGET" = windows ] || [ "$TARGET" = all ]; then
  run "npx --yes @tauri-apps/cli@2 build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis"
  cp target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe "$OUT"/
fi

(cd "$OUT" && rm -f SHA256SUMS && sha256sum -- * > SHA256SUMS)
ls -lh "$OUT"
