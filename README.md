# JenderalCode Desktop

**Aplikasi desktop AI coding agent** — bangun, edit, uji, dan kelola proyek perangkat lunak melalui satu workspace dengan dukungan berbagai provider AI, model lokal, dan coding agent eksternal.

Dibangun sesuai *JenderalCode-Desktop-PRD.md* v1.0 (MVP).

![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-blue)
![Stack](https://img.shields.io/badge/stack-Rust%20%2B%20Tauri%202%20%2B%20React%20%2B%20TypeScript-orange)

## Prinsip utama

- **One workspace, many agents** — berpindah provider dan agent tanpa berpindah aplikasi.
- **Native desktop experience** — Rust untuk operasi sistem dan runtime agent.
- **Model-agnostic** — tidak terkunci pada satu vendor AI.
- **Human-in-the-loop** — tinjau diff dan setujui tindakan sensitif.
- **Local-first** — proyek tetap di perangkat; data hanya dikirim ke provider sesuai konfigurasi.

## Fitur (MVP)

| Area | Yang bisa Anda lakukan |
|---|---|
| **Project workspace** | Buka folder lokal, deteksi Git, ingat workspace terakhir (FR-01) |
| **Provider manager** | Tambah/edit/uji/aktifkan/hapus provider; tes koneksi; daftar model + capability (FR-02) |
| **Model selector** | Pilih provider/model per sesi (FR-03) |
| **AI chat** | Streaming respons, markdown, code block, pembatalan (FR-04) |
| **File context** | Lampirkan file ke chat via tombol attach / drag-and-drop; instruksi proyek (`AGENTS.md`, `CLAUDE.md`, …) otomatis jadi konteks (FR-05) |
| **Agent tools** | Baca, cari (gitignore-aware), buat, edit file; jalankan tes lewat terminal (FR-06) |
| **Terminal** | Terminal interaktif penuh (PTY + xterm.js) untuk Anda, dan tool `terminal_run` (timeout, output cap, process-group kill) untuk agent (FR-07) |
| **Diff review** | Diff viewer unified/split; preview diff pada kartu approval; panel Git (status/stage/commit/branch/log) (FR-08) |
| **Session history** | Riwayat percakapan per workspace, pencarian, lanjutkan sesi lama (FR-09) |
| **Agent profiles** | Mode Plan / Build / Review + profil custom (prompt, tools, policy) (FR-10) |
| **Git panel** | Status, diff, staging, commit, branch, discard (FR-11) |
| **Usage dashboard** | Token, estimasi biaya (pricing per model bisa diatur), grafik 30 hari (FR-15) |
| **Mode Chat / Code** | Mode **Chat** (default): daftar percakapan, chat lebar, panel konteks (Changes/Files/Editor/Terminal/Git). Mode **Code**: layout IDE dengan chat di samping. Ganti lewat toggle di header proyek atau ⌘K; pilihan terakhir diingat |
| **Browser preview** | Tab **Browser** (panel konteks mode Chat / panel bawah mode Code) untuk membuka `localhost:3000` dsb. — webview native terisolasi (tanpa akses IPC, cookie terpisah), back/forward/reload, URL terakhir per proyek diingat |
| **Token & konteks** | Meter di kotak chat: persentase jendela konteks terpakai + total token input/output, jumlah request, dan estimasi biaya per percakapan |
| **Zoom UI** | Zoom in/out dengan ⌘+/⌘− (Ctrl+/Ctrl− di Windows/Linux), reset ⌘0; level tersimpan dan dipulihkan saat aplikasi dibuka |
| **Keamanan** | Permission engine (Safe/Balanced/Yolo), approval card, checkpoint restore, API key di OS keychain, proteksi path traversal & symlink escape, file sensitif (.env dsb.) minta persetujuan (kecuali mode Full access) |

### Remote dari HP (JenderalCode Mobile)

Kendalikan sesi dari HP Android: chat, approval, ganti agent/model/mode,
Files, Editor, Changes, Git, Terminal, serta sesi agent CLI (Claude Code,
Codex, OpenCode, Antigravity). Pesan terenkripsi end-to-end antara HP dan
desktop; relay hanya meneruskan (`docs/REMOTE_PROTOCOL.md`).

- Unduh app Android: <https://github.com/mohammadirham37/jenderalcode_mobile/releases/latest>
- Di desktop: **Settings → Remote control** → nyalakan, lalu **Pair a phone** dan scan QR dari app.
- Full access, terminal dan edit file/git dari HP baru aktif setelah
  **Allow Full access & terminal from phone** dinyalakan di desktop.

### Provider yang didukung

- **OpenAI-compatible** — OpenAI, DeepSeek, Mistral, xAI, Groq, gateway/proxy kustom.
- **Anthropic** — Messages API dengan tool use.
- **Google Gemini** — `streamGenerateContent` dengan function calling.
- **Local inference** — Ollama & LM Studio (keduanya OpenAI-compatible; preset tersedia).
- **Mock (demo)** — provider bawaan tanpa API key/network; menjalankan alur agent lengkap termasuk tool call + approval. Cocok untuk mencoba aplikasi dan untuk testing otomatis.
- **External CLI agents** — adapter generik experimental (Claude Code / Codex / OpenCode preset).

## Arsitektur

```
jenderalcode-desktop/
├── apps/desktop/          # Aplikasi Tauri 2 (commands, events, PTY, watcher)
├── crates/
│   ├── agent-runtime/     # Agent loop, context manager, approval, cancellation, usage
│   ├── provider-core/     # Trait AiProvider, registry, credential store (keychain), mock
│   ├── provider-openai/   # Adapter OpenAI-compatible (streaming + tool calling)
│   ├── provider-anthropic/# Adapter Anthropic Messages API
│   ├── provider-google/   # Adapter Google Gemini
│   ├── agent-cli/         # Adapter external coding agent CLI (experimental)
│   ├── tool-filesystem/   # read/write/edit/search + checkpoint + path safety
│   ├── tool-terminal/     # Exec manager: timeout, output cap, process-group kill
│   ├── tool-git/          # Operasi Git via CLI
│   ├── permissions/       # Policy engine (Safe/Balanced/Yolo, grants, deny-list)
│   ├── storage/           # SQLite (bundled) + migrasi
│   └── common/            # Tipe & event bersama
├── frontend/              # React + TypeScript + Vite + Tailwind CSS 4
│   └── src/features/      # workspace, chat, terminal, diff, providers, agents, sessions, settings
├── docs/                  # Dokumentasi teknis
└── tests/                 # Test e2e lintas crate
```

Setiap tool agent melewati **permission engine** dan menghasilkan event yang dirender di UI. Tulisan sebelum eksekusi: file di-snapshot ke **checkpoint** sehingga semua edit agent bisa di-rollback satu klik.

## Menjalankan dari source

Prasyarat:

- Rust 1.85+ (`rustup`)
- Node.js 20+ dan npm
- Git (untuk panel Git dan fitur terkait)

```bash
# 1. Install dependency frontend
cd frontend && npm install && cd ..

# 2. Jalankan mode development (vite + tauri)
#    Pastikan Tauri CLI tersedia; npx akan mengunduhnya otomatis.
npx --yes @tauri-apps/cli@2 dev          # dari root, cwd apps/desktop

# atau manual:
cd apps/desktop
npx --yes @tauri-apps/cli@2 dev

# 3. Build bundle (macOS: .app + .dmg)
npx --yes @tauri-apps/cli@2 build

#    Jika langkah DMG gagal karena AppleEvent Finder timeout (umum di
#    lingkungan non-interaktif), build hanya .app:
npx --yes @tauri-apps/cli@2 build --bundles app
```

Hasil build ada di `target/release/bundle/macos/JenderalCode.app` (atau
`target/debug/...` untuk `--debug`).

### Distribusi ke Mac lain

Build universal (Intel + Apple Silicon), ditandatangani ad-hoc lewat
`bundle.macOS.signingIdentity: "-"`:

```bash
rustup target add x86_64-apple-darwin aarch64-apple-darwin
cd apps/desktop && npx --yes @tauri-apps/cli@2 build --target universal-apple-darwin
# → target/universal-apple-darwin/release/bundle/dmg/JenderalCode_<versi>_universal.dmg
```

Bagikan file **.dmg** (jangan build `--debug`, dan jangan menyalin `.app` mentah).
Karena belum dinotarisasi Apple, di Mac tujuan pertama kali buka dengan klik kanan
→ **Open**, atau hapus atribut karantina:

```bash
xattr -cr /Applications/JenderalCode.app
```

Untuk distribusi tanpa peringatan Gatekeeper, perlu sertifikat *Developer ID* +
notarisasi (set `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` saat build).

Data aplikasi tersimpan di:

- macOS: `~/Library/Application Support/com.jenderalcode.desktop/`
- Linux: `~/.local/share/com.jenderalcode.desktop/` (XDG)
- Windows: `%APPDATA%\com.jenderalcode.desktop\`

Berisi: `jenderalcode.db` (SQLite), `checkpoints/`, dan `credentials.json` (API key, permission 0600 — default; bisa dipindah ke OS keychain lewat **Settings → Privacy**).

## Testing

```bash
cargo test --workspace     # semua unit + integrasi test crate
cd frontend && npm run build   # type-check (tsc) + build
```

Cakupan test (PRD §13): permission matrix, path traversal & symlink escape, edit-uniqueness, checkpoint restore, migrasi & retensi storage, SSE parsing provider, konversi pesekan lintas protokol, exec timeout/kill/output cap, alur approval penuh runtime dengan mock provider (approve/deny/grant/cancel), dan status/stage/commit/log Git.

## Struktur data lokal

SQLite (`jenderalcode.db`) menyimpan metadata aplikasi sesuai PRD §11: `workspaces`, `providers`, `models`, `agent_profiles`, `sessions`, `messages`, `tool_runs`, `approvals`, `usage_records`, `settings`. **API key tidak pernah disimpan di database** — hanya referensi ke OS keychain (`security-framework` / Credential Manager / Secret Service).

## Keamanan

- API key disimpan **di luar database**: default berupa file khusus dengan permission 0600 (tanpa prompt password OS sama sekali), atau OS keychain (macOS Keychain / Windows Credential Manager / Secret Service) bila dipilih di **Settings → Privacy** — key yang sudah tersimpan dimigrasi otomatis saat berganti.
- Semua path dari model divalidasi: canonicalize + containment terhadap root workspace; symlink escape ditolak; penulisan di luar workspace selalu ditolak.
- File sensitif (`.env`, `*.pem`, `id_rsa`, …) selalu meminta persetujuan sebelum dibaca.
- Perintah shell dianalisis per segmen (`;`, `|`, `&&`, `$(…)`, `bash -c`, …): perintah destruktif (`rm -rf /`, `rm -fr ~`, `mkfs`, …) ditolak mutlak; git push/pull/fetch/remote, `sudo`, dan pola download-lalu-eksekusi (`curl … | sh`) minta persetujuan di Safe/Balanced. Mode Balanced meminta persetujuan untuk setiap perintah. Mode **Full access** (Yolo) tidak pernah meminta persetujuan; hanya perintah katastrofik dan penulisan di luar proyek yang tetap ditolak.
- Penulisan file tidak pernah mengikuti symlink (O_NOFOLLOW); pencarian melewati symlink dan file rahasia.
- Webview dikunci dengan CSP ketat dan pembatasan navigasi; output model tidak bisa memuat gambar/URL eksternal.
- Variabel lingkungan rahasia (`*_API_KEY`, `*_TOKEN`, …) dihapus dari proses terminal dan perintah agent.
- Proses agent: timeout, batas output, dan process-group kill.
- Instruksi proyek (AGENTS.md dll.) diperlakukan sebagai data tidak tepercaya.
- Checkpoint per sesi untuk memulihkan file sebelum edit agent.

## Roadmap pasca-MVP

Mengikuti PRD: integrasi MCP & LSP (P1), multi-agent workflow & plugin system (P2), dan adapter external agent yang lebih dalam.

## Lisensi

MIT
