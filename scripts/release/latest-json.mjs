#!/usr/bin/env node
// Writes the updater feed (latest.json) for a release from the signed
// artifacts in dist-release/<version>/:
//
//   node scripts/release/latest-json.mjs <version> [notes-file]
//
// Platforms are included when their update bundle and its .sig exist:
//   macOS   JenderalCode_<v>_universal.app.tar.gz  -> darwin-aarch64, darwin-x86_64
//   Linux   *.AppImage                              -> linux-x86_64
//   Windows *-setup.exe (NSIS)                      -> windows-x86_64
// The app fetches releases/latest/download/latest.json (tauri.conf.json).
import { createHash, createPublicKey, verify } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const REPO = process.env.JC_RELEASE_REPO ?? "mohammadirham37/jenderal_code_desktop";
const [version, notesFile] = process.argv.slice(2);
if (!version || !/^\d+\.\d+\.\d+/.test(version)) {
  console.error("usage: latest-json.mjs <version> [notes-file]");
  process.exit(1);
}
const dir = join("dist-release", version);
const files = existsSync(dir) ? readdirSync(dir) : [];
const url = (file) => `https://github.com/${REPO}/releases/download/v${version}/${encodeURIComponent(file)}`;

// The app only installs bundles signed by the key behind this pubkey, so a
// bundle signed with another key would be offered but never install.
const conf = JSON.parse(readFileSync("apps/desktop/tauri.conf.json", "utf8"));
const minisignLine = (b64) => Buffer.from(b64, "base64").toString("utf8").split("\n")[1];
const pub = Buffer.from(minisignLine(conf.plugins.updater.pubkey), "base64"); // "Ed" | key id | key
const publicKey = createPublicKey({
  key: { kty: "OKP", crv: "Ed25519", x: pub.subarray(10).toString("base64url") },
  format: "jwk",
});

/** Check a Tauri (minisign) signature of `file` against the pubkey. */
function checkSignature(file, signature) {
  const sig = Buffer.from(minisignLine(signature), "base64"); // "ED" | key id | signature
  if (!sig.subarray(2, 10).equals(pub.subarray(2, 10))) {
    throw new Error(`${file}: signed with another key than tauri.conf.json's pubkey`);
  }
  const data = readFileSync(join(dir, file));
  const message = sig.subarray(0, 2).toString() === "ED" ? createHash("blake2b512").update(data).digest() : data;
  if (!verify(null, message, publicKey, sig.subarray(10))) throw new Error(`${file}: bad signature`);
}

const platforms = {};
const add = (keys, match) => {
  const file = files.find((f) => match(f) && files.includes(`${f}.sig`));
  if (!file) return;
  const signature = readFileSync(join(dir, `${file}.sig`), "utf8").trim();
  checkSignature(file, signature);
  const entry = { signature, url: url(file) };
  for (const key of keys) platforms[key] = entry;
};
add(["darwin-aarch64", "darwin-x86_64"], (f) => f.endsWith(".app.tar.gz"));
add(["linux-x86_64"], (f) => f.endsWith(".AppImage"));
add(["windows-x86_64"], (f) => f.endsWith("-setup.exe"));

if (Object.keys(platforms).length === 0) {
  console.error(`no signed update bundles in ${dir} (build with the release scripts and TAURI_SIGNING_PRIVATE_KEY set)`);
  process.exit(1);
}

const feed = {
  version,
  notes: notesFile ? readFileSync(notesFile, "utf8").trim() : "",
  pub_date: new Date().toISOString(),
  platforms,
};
writeFileSync(join(dir, "latest.json"), JSON.stringify(feed, null, 2) + "\n");
console.log(`${join(dir, "latest.json")}: ${Object.keys(platforms).join(", ")}`);
