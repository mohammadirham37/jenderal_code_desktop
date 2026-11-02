# Panduan Rilis (untuk agen dan maintainer)

Dokumen ini publik. **Jangan pernah** menuliskan data pribadi di sini atau di
file lain di repo: alamat IP/hostname server, nama user SSH, path folder lokal
atau remote, token, password, API key, isi `~/.ssh`, atau sertifikat
penandatanganan. Pakai placeholder seperti `<build-host>` dan `<user>`.
Detail mesin build disimpan di catatan pribadi maintainer, bukan di repo.

## Ringkasan target

| OS | Bundle | Dibangun di |
|----|--------|-------------|
| macOS (Apple Silicon + Intel) | `.app`, `.dmg` (universal) | Mac |
| Linux x86_64 | `.deb`, `.rpm`, `.AppImage` | Host Linux + Docker |
| Windows x86_64 | installer NSIS `.exe` | Host Linux + Docker (cross-compile `cargo-xwin`) |

Bundle macOS tidak bisa dibangun di Linux; bundle Linux dan Windows paling
mudah dibangun di Docker (`scripts/release/`).

## 1. Persiapan versi

1. Pastikan `main` bersih dan lolos cek:
   ```bash
   cargo test --workspace
   npm --prefix frontend run build
   ```
2. Naikkan versi (wajib untuk setiap rilis: updater hanya menawarkan versi
   yang lebih tinggi dari yang terpasang):
   ```bash
   scripts/release/bump-version.sh X.Y.Z
   ```
   Skrip ini mengubah `Cargo.toml` (`[workspace.package] version`),
   `apps/desktop/tauri.conf.json`, `frontend/package.json` dan kedua lockfile.
3. Commit (`Release vX.Y.Z`). Tag `vX.Y.Z` dibuat oleh `publish.sh`.

## Update otomatis (OTA)

App memeriksa `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`
(lihat `plugins.updater` di `tauri.conf.json`) saat dibuka lalu tiap 6 jam,
dan menampilkan tombol update di sidebar serta di Settings → Updates.
Bundle update ditandatangani (minisign); app hanya memasang bundle yang
cocok dengan `pubkey` di `tauri.conf.json`.

- **Kunci privat** disimpan maintainer di luar repo (default
  `~/.tauri/jenderalcode-updater.key`) dan dicadangkan di password manager.
  Kalau hilang, app yang sudah terpasang tidak bisa menerima update lagi
  (pengguna harus memasang ulang versi dengan kunci baru). Kalau bocor,
  orang lain bisa membuat update palsu: buat kunci baru dan ganti `pubkey`.
- Build biasa (`tauri build`) tidak butuh kunci. Hanya skrip rilis yang
  mengaktifkan `createUpdaterArtifacts` (lewat `--config`) dan membutuhkan
  `TAURI_SIGNING_PRIVATE_KEY` (isi kunci atau path ke file di Mac;
  **isi kunci** untuk build Docker) serta
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (kosong bila kunci tanpa password).
  Berikan lewat environment sesi itu saja; jangan tulis ke file di repo
  atau ke shell history di host bersama.
- Bundle update per OS: macOS `JenderalCode_X.Y.Z_universal.app.tar.gz`,
  Linux `.AppImage` (paket `.deb`/`.rpm` tidak di-update otomatis),
  Windows installer NSIS `-setup.exe`. Masing-masing punya `.sig`, yang
  isinya masuk ke `latest.json` (file `.sig` sendiri tidak diunggah).

## 2. Linux dan Windows (host Linux dengan Docker)

Salin source ke host build. Kalau host tidak punya akses ke repo, kirim
lewat git bundle, sehingga yang terkirim hanya riwayat yang sudah di-commit
(tanpa file lokal yang tidak di-track):

```bash
git bundle create /tmp/jc.bundle main
scp /tmp/jc.bundle <user>@<build-host>:/tmp/
ssh <user>@<build-host> 'git clone -b main /tmp/jc.bundle <folder-build> && rm /tmp/jc.bundle'
```

Untuk rilis berikutnya cukup `git pull /tmp/jc.bundle main` di folder yang sama.

Lalu jalankan di host, dengan kunci updater di environment:

```bash
cd <folder-build>
read -rs TAURI_SIGNING_PRIVATE_KEY && export TAURI_SIGNING_PRIVATE_KEY   # tempel isi kunci
scripts/release/build-linux-windows.sh all   # atau: linux | windows
unset TAURI_SIGNING_PRIVATE_KEY
```

Salin hasil `dist-release/<versi>/` (termasuk file `.sig`) ke folder yang
sama di Mac sebelum publikasi.

- Image `jenderalcode-release-builder` (Ubuntu 22.04, Rust stable, Node 22,
  webkit2gtk, clang/lld, NSIS, cargo-xwin) dibangun otomatis.
- CPU dan memori container dibatasi (`JC_CPUS`, default 4; `JC_MEMORY`,
  default 5g) supaya tidak mengganggu layanan lain di host bersama.
- Hasil: `dist-release/<versi>/` plus `SHA256SUMS` (folder ini di-ignore git).
- Build Windows memakai MSVC SDK yang diunduh `cargo-xwin` (cache di volume
  Docker `jenderalcode-cache`, bersama cache cargo dan npm). Installer NSIS mengunduh WebView2 saat
  instalasi bila belum ada.

## 3. macOS (di Mac)

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
scripts/release/build-macos.sh
```

Skrip ini membangun `.app` dan `.dmg` universal plus bundle update
(`.app.tar.gz` + `.sig`), memverifikasi DMG, dan menyalinnya ke
`dist-release/<versi>/`. Kuncinya diambil dari `TAURI_SIGNING_PRIVATE_KEY`
atau file `~/.tauri/jenderalcode-updater.key` (`JC_UPDATER_KEY` untuk lokasi
lain). Build tanpa updater (misalnya untuk dicoba sendiri) tetap bisa dengan
`npx --yes @tauri-apps/cli@2 build --target universal-apple-darwin --bundles app,dmg`;
hasilnya ada di `target/universal-apple-darwin/release/bundle/{macos,dmg}/`.
Kalau pembuatan DMG gagal karena AppleEvent Finder timeout (-1712), `.app`
sudah jadi. Lepas dulu image sementara yang masih ter-mount (`hdiutil info`,
lalu `hdiutil detach <disk>`) dan hapus `bundle/macos/rw.*.dmg`, kemudian buat
DMG langsung dengan `hdiutil` (tanpa tata letak jendela Finder):

```bash
B=target/universal-apple-darwin/release/bundle
STAGE=$(mktemp -d)
cp -R "$B/macos/JenderalCode.app" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname JenderalCode -srcfolder "$STAGE" -fs HFS+ -format UDZO \
  -ov "$B/dmg/JenderalCode_<versi>_universal.dmg"
rm -rf "$STAGE"
hdiutil verify "$B/dmg/JenderalCode_<versi>_universal.dmg"
lipo -archs "$B/macos/JenderalCode.app/Contents/MacOS/jenderalcode-desktop"   # harus: x86_64 arm64
```

Build universal butuh sekitar 1,5 GB ruang kosong (dua target ~575 MB
masing-masing). Kalau disk sempit, build `--target aarch64-apple-darwin` saja.

Saat ini app hanya ditandatangani ad-hoc (`signingIdentity: "-"`), tanpa
notarisasi. Pengguna harus membukanya lewat klik kanan → Open, atau
`xattr -dr com.apple.quarantine /Applications/JenderalCode.app`. Untuk
notarisasi butuh Apple Developer ID. Kredensialnya diberikan lewat environment
(`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`),
**tidak pernah** di-commit.

Windows juga belum ditandatangani (SmartScreen akan memperingatkan).

## 4. Publikasi

1. Kumpulkan semua artefak (termasuk `.sig`) di `dist-release/X.Y.Z/`.
2. Uji minimal: app terbuka, bisa membuka workspace, dan satu chat dengan
   provider mock/nyata berjalan, di tiap OS yang tersedia.
3. Tulis catatan rilis (ditampilkan apa adanya di dialog update app), lalu:
   ```bash
   scripts/release/publish.sh <catatan.md>
   ```
   Skrip ini membuat `latest.json` (`scripts/release/latest-json.mjs`),
   membuat dan mem-push tag `vX.Y.Z`, lalu `gh release create` dengan semua
   artefak dan `latest.json`. Rilis harus berstatus *latest* (bukan draft
   atau prerelease), karena app membaca `releases/latest`.
   Push dan publikasi rilis adalah aksi keluar: agen harus minta persetujuan
   maintainer dulu.
4. Cek: `curl -sL https://github.com/<owner>/<repo>/releases/latest/download/latest.json`
   menampilkan versi baru. App yang terpasang menawarkan update paling lambat
   6 jam kemudian (atau lewat Settings → Updates → Check for updates).

## Aturan untuk agen

- Jangan menulis host, user, IP, path home/remote, atau kredensial ke file
  repo, pesan commit, catatan rilis, maupun log yang di-commit.
- Jangan commit artefak build (`target/`, `dist-release/`) atau kunci updater
  (`*.key`); hanya `pubkey` di `tauri.conf.json` yang boleh masuk repo.
- Sebelum commit, cek: `git diff --cached | grep -nE '[0-9]{1,3}(\.[0-9]{1,3}){3}|/home/|/Users/|BEGIN .*PRIVATE KEY|ssh-(ed25519|rsa)'`
  harus kosong.
- Host build bersama bisa menjalankan layanan produksi. Jangan menghentikan,
  me-restart, atau menghapus container/volume lain; gunakan hanya image dan
  volume berprefiks `jenderalcode-`.
