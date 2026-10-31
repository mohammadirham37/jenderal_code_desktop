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
2. Naikkan versi di tiga tempat sekaligus (harus sama):
   - `Cargo.toml` → `[workspace.package] version`
   - `apps/desktop/tauri.conf.json` → `version`
   - `frontend/package.json` → `version` (lalu `npm --prefix frontend install` agar lockfile ikut)
3. Commit (`Release vX.Y.Z`) dan buat tag `vX.Y.Z`.

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

Lalu jalankan di host:

```bash
cd <folder-build>
scripts/release/build-linux-windows.sh all   # atau: linux | windows
```

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
npx --yes @tauri-apps/cli@2 build --target universal-apple-darwin --bundles app,dmg
```

Hasil ada di `target/universal-apple-darwin/release/bundle/{macos,dmg}/`.
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

1. Kumpulkan semua artefak ke satu folder dan buat ulang `SHA256SUMS`.
2. Uji minimal: app terbuka, bisa membuka workspace, dan satu chat dengan
   provider mock/nyata berjalan, di tiap OS yang tersedia.
3. Push tag dan buat GitHub Release:
   ```bash
   git push origin vX.Y.Z
   gh release create vX.Y.Z dist-release/X.Y.Z/* --title "vX.Y.Z" --notes-file <catatan>
   ```
   Push dan publikasi rilis adalah aksi keluar: agen harus minta persetujuan
   maintainer dulu.

## Aturan untuk agen

- Jangan menulis host, user, IP, path home/remote, atau kredensial ke file
  repo, pesan commit, catatan rilis, maupun log yang di-commit.
- Jangan commit artefak build (`target/`, `dist-release/`).
- Sebelum commit, cek: `git diff --cached | grep -nE '[0-9]{1,3}(\.[0-9]{1,3}){3}|/home/|/Users/|BEGIN .*PRIVATE KEY|ssh-(ed25519|rsa)'`
  harus kosong.
- Host build bersama bisa menjalankan layanan produksi. Jangan menghentikan,
  me-restart, atau menghapus container/volume lain; gunakan hanya image dan
  volume berprefiks `jenderalcode-`.
