#!/usr/bin/env bash
# Sets the app version everywhere it must match (the updater compares it):
#
#   scripts/release/bump-version.sh X.Y.Z
set -euo pipefail
V=${1:?usage: bump-version.sh X.Y.Z}
[[ $V =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a version: $V" >&2; exit 1; }
cd "$(dirname "$0")/../.."
node -e '
  const fs = require("fs");
  // The first `version = ` is [workspace.package] (crates inherit it).
  const cargo = fs.readFileSync("Cargo.toml", "utf8");
  fs.writeFileSync("Cargo.toml", cargo.replace(/^version = "[^"]+"/m, `version = "${process.argv[1]}"`));
  for (const p of ["apps/desktop/tauri.conf.json", "frontend/package.json", "frontend/package-lock.json"]) {
    const j = JSON.parse(fs.readFileSync(p, "utf8"));
    j.version = process.argv[1];
    // The lockfile repeats it for the root package; nothing else changes.
    if (j.packages?.[""]) j.packages[""].version = process.argv[1];
    fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n");
  }' "$V"
cargo update -w --offline >/dev/null 2>&1 || cargo update -w >/dev/null
grep -n "^version" Cargo.toml | head -1
grep -n '"version"' apps/desktop/tauri.conf.json frontend/package.json
