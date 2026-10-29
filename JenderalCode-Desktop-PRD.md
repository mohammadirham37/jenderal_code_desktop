# PRD — JenderalCode Desktop

**Versi:** 1.0  
**Status:** Draft untuk pengembangan MVP  
**Jenis produk:** Aplikasi desktop AI coding agent  
**Platform target:** macOS, Windows, Linux

---

## 1. Ringkasan Produk

**JenderalCode Desktop** adalah aplikasi desktop AI coding agent yang memungkinkan developer membangun, mengedit, menguji, dan mengelola proyek perangkat lunak melalui satu workspace dengan dukungan berbagai provider AI, model lokal, dan coding agent eksternal.

Aplikasi ini terinspirasi dari pengalaman coding agent seperti OpenCode, tetapi dirancang sebagai desktop application dengan antarmuka visual terpadu. Pengguna dapat membuka proyek lokal, memilih model AI, berdiskusi dengan agent, meninjau perubahan kode, menjalankan perintah terminal, dan mengelola riwayat sesi.

### Prinsip utama

- **One workspace, many agents:** berpindah provider dan agent tanpa berpindah aplikasi.
- **Native desktop experience:** menggunakan Rust untuk operasi sistem dan runtime agent.
- **Model-agnostic:** tidak terkunci pada satu vendor AI.
- **Human-in-the-loop:** pengguna dapat meninjau perubahan dan menyetujui tindakan sensitif.
- **Local-first:** proyek tetap berada di perangkat pengguna; data hanya dikirim ke layanan eksternal sesuai konfigurasi dan tindakan pengguna.

## 2. Tujuan Produk

### 2.1 Tujuan

1. Menyediakan satu aplikasi desktop untuk bekerja dengan berbagai provider AI.
2. Memungkinkan agent memahami struktur proyek dan konteks kode lokal.
3. Memungkinkan agent membaca, membuat, mengedit, dan mencari file.
4. Memungkinkan agent menjalankan perintah dan tes melalui terminal dengan kontrol izin.
5. Menyediakan diff viewer untuk memeriksa perubahan kode.
6. Menyimpan riwayat percakapan dan konfigurasi workspace secara lokal.
7. Mendukung integrasi model melalui API serta agent coding eksternal melalui adapter.

### 2.2 Bukan tujuan MVP

- Kolaborasi real-time antarpengguna.
- Cloud sync dan akun pengguna.
- Marketplace plugin penuh.
- Multi-agent paralel yang mengubah file secara bersamaan.
- Mendukung semua provider dan semua protokol agent sejak versi pertama.

## 3. Target Pengguna

| Pengguna | Kebutuhan |
|---|---|
| Software developer | Membuat fitur, memperbaiki bug, refactoring, dan menulis tes |
| AI-assisted developer | Memilih model berdasarkan kualitas, latensi, dan biaya |
| Tech lead | Meninjau diff dan menjaga kualitas kode |
| Mahasiswa dan pengajar | Belajar pemrograman dan mengembangkan proyek |
| Tim software house | Mengelola banyak proyek dan konfigurasi agent |

## 4. Technology Stack

| Lapisan | Teknologi yang direkomendasikan |
|---|---|
| Desktop runtime | Rust + Tauri 2 |
| Frontend | React + TypeScript + Vite |
| Styling | Tailwind CSS dan komponen UI konsisten |
| Code editor | Monaco Editor atau CodeMirror 6 |
| Terminal UI | xterm.js |
| Terminal process | PTY crate yang kompatibel dengan target OS |
| Database lokal | SQLite + SQLx atau rusqlite |
| HTTP dan streaming | Rust async runtime + HTTP client |
| File watching | `notify` crate |
| Git | Git CLI sebagai implementasi awal |
| Credential storage | macOS Keychain, Windows Credential Manager, Linux Secret Service bila tersedia |

Stack final perlu divalidasi melalui proof of concept, terutama integrasi PTY, packaging lintas OS, dan lisensi dependensi.

## 5. Arsitektur Sistem

Arsitektur memisahkan UI, agent runtime, provider adapter, dan tool execution.

```text
JenderalCode Desktop
├── Frontend (React + TypeScript)
│   ├── Workspace & File Explorer
│   ├── AI Chat
│   ├── Code Editor
│   ├── Terminal
│   ├── Diff Viewer
│   ├── Provider Center
│   └── Session History
│
├── Tauri Commands / Events
│
├── Rust Core
│   ├── Agent Runtime
│   ├── Context Manager
│   ├── Provider Registry
│   ├── Permission Engine
│   ├── Session Manager
│   ├── Storage
│   └── Process Manager
│
├── Provider & Agent Adapters
│   ├── Native Provider APIs
│   ├── OpenAI-Compatible API
│   ├── Local Model Endpoints
│   └── External Coding Agent CLI
│
└── Tools
    ├── Filesystem
    ├── Search
    ├── Terminal
    ├── Git
    ├── LSP (fase lanjutan)
    └── MCP (fase lanjutan)
```

### 5.1 Prinsip arsitektur

- Provider API dan coding agent adalah dua konsep berbeda.
- Provider menyediakan model/API; agent mengelola alur kerja, konteks, tools, dan penyelesaian tugas.
- Direct API sebaiknya memakai tool runtime milik JenderalCode agar izin dan perubahan kode dapat dikontrol secara konsisten.
- External CLI dijalankan sebagai proses terpisah melalui adapter khusus. Jangan menganggap semua CLI memiliki protokol yang sama.
- Seluruh operasi tool harus melalui permission engine dan menghasilkan event yang dapat ditampilkan di UI.
- Model dengan kemampuan berbeda harus ditangani melalui capability detection, bukan diasumsikan mendukung seluruh fitur.

## 6. Provider dan Agent Integration

### 6.1 Kategori integrasi

**Cloud AI providers**
- OpenAI
- Anthropic
- Google Gemini
- xAI
- Mistral
- DeepSeek
- Provider lain yang API-nya didukung

**OpenAI-compatible endpoints**
- Gateway AI
- Proxy internal
- API kompatibel dari penyedia pihak ketiga
- Endpoint kustom yang dikonfigurasi pengguna

**Local inference**
- Ollama
- LM Studio
- Server model lokal lain dengan API yang didukung

**External coding agents**
- OpenCode CLI
- Codex CLI
- Claude Code
- CLI agent lain apabila tersedia mekanisme integrasi yang didukung

Daftar ini merupakan target integrasi. Kompatibilitas aktual, autentikasi, lisensi, dan dukungan protokol harus diverifikasi ketika masing-masing adapter dikembangkan.

### 6.2 Direct provider vs external agent

| Aspek | Direct provider | External agent |
|---|---|---|
| Komunikasi | API HTTP/streaming | Proses CLI atau protokol resmi |
| Tool execution | Dikendalikan runtime JenderalCode | Dapat dikendalikan agent eksternal |
| File editing | Melalui tool JenderalCode | Mengikuti mekanisme agent eksternal |
| Permission | Dapat dipusatkan di runtime | Perlu disesuaikan dengan mekanisme agent |
| Kompatibilitas | Bergantung pada API/model | Bergantung pada versi dan protokol CLI |

**Strategi MVP:** mulai dengan provider API langsung dan OpenAI-compatible API. Tambahkan satu external coding agent adapter setelah alur utama stabil.

### 6.3 Provider registry

Setiap konfigurasi provider menyimpan:

- ID dan nama provider.
- Tipe adapter.
- Base URL.
- Referensi credential aman.
- Daftar model atau mekanisme pemuatan model.
- Capability model.
- Status koneksi dan pesan error.
- Parameter khusus provider.

Kapabilitas model dapat meliputi streaming, tool calling, vision, structured output, context window, dan batas output. Informasi yang tidak diketahui harus ditandai sebagai tidak diketahui, bukan ditebak.

### 6.4 Provider management flow

1. Pengguna membuka **Settings → Providers**.
2. Memilih provider bawaan atau menambahkan custom endpoint.
3. Memasukkan API key atau memilih metode autentikasi yang didukung.
4. Menjalankan tes koneksi.
5. Memilih model yang tersedia.
6. Memilih model untuk sesi baru atau mengganti model pada sesi yang sudah ada.
7. Aplikasi menampilkan error autentikasi, endpoint, rate limit, atau kemampuan yang tidak didukung secara jelas.

## 7. Fitur Fungsional

Prioritas:
- **P0:** wajib untuk MVP.
- **P1:** pengembangan setelah MVP inti.
- **P2:** fitur lanjutan.

| ID | Fitur | Deskripsi | Prioritas |
|---|---|---|---|
| FR-01 | Project workspace | Membuka folder lokal, mendeteksi Git, dan mengingat workspace terakhir | P0 |
| FR-02 | Provider manager | Tambah, edit, uji, aktifkan, dan hapus provider | P0 |
| FR-03 | Model selector | Memilih provider/model untuk setiap sesi | P0 |
| FR-04 | AI chat | Streaming respons, markdown, code block, dan pembatalan | P0 |
| FR-05 | File context | Melampirkan file, folder, selection, dan instruksi proyek | P0 |
| FR-06 | Agent tools | Membaca, mencari, membuat, dan mengedit file | P0 |
| FR-07 | Terminal | Menjalankan command, menampilkan output, dan menghentikan proses | P0 |
| FR-08 | Diff review | Meninjau perubahan per file dan per hunk | P0 |
| FR-09 | Session history | Menyimpan dan membuka percakapan per workspace | P0 |
| FR-10 | Agent profiles | Plan, Build, Review, dan profil custom | P1 |
| FR-11 | Git panel | Status, diff, staging, commit, dan branch | P1 |
| FR-12 | MCP integration | Menghubungkan tools eksternal melalui MCP | P1 |
| FR-13 | LSP integration | Diagnostics dan code intelligence | P1 |
| FR-14 | Multi-agent workflow | Delegasi dan penggabungan hasil agent | P2 |
| FR-15 | Usage dashboard | Token, estimasi biaya, durasi, dan penggunaan | P1 |
| FR-16 | Plugin system | Ekstensi provider, tools, dan workflow | P2 |

## 8. Agent Workflow

### 8.1 Alur utama

1. Pengguna membuka proyek lokal dan memilih model.
2. Pengguna memberikan instruksi, misalnya: “Buat CRUD produk dengan validasi dan unit test.”
3. Agent menganalisis struktur proyek, instruksi, dependensi, dan file terkait.
4. Agent menyusun rencana dan daftar file yang akan diubah.
5. Runtime menjalankan tools yang diizinkan.
6. Tindakan sensitif meminta persetujuan pengguna sesuai kebijakan.
7. Perubahan ditampilkan di diff viewer.
8. Pengguna menerima atau menolak perubahan.
9. Agent menjalankan tes yang relevan dan melaporkan hasil serta pekerjaan yang belum selesai.

### 8.2 Mode agent

**Plan Mode**
- Menganalisis proyek dan menyusun rencana.
- Tidak mengubah file atau menjalankan perintah yang mengubah sistem.
- Menghasilkan langkah implementasi yang dapat ditinjau.

**Build Mode**
- Mengimplementasikan fitur.
- Membaca dan mengedit file.
- Menjalankan tes dan memperbaiki kesalahan.
- Mengikuti permission policy.

**Review Mode**
- Memeriksa bug, keamanan, performa, dan kualitas kode.
- Akses tulis dinonaktifkan secara default.
- Menghasilkan temuan yang disertai lokasi file dan alasan.

**Custom Agent**
- Pengguna dapat menentukan prompt, model, daftar tools, dan kebijakan izin.

## 9. Spesifikasi UI/UX

### 9.1 Halaman

1. **Welcome / Projects**
   - Daftar proyek terakhir.
   - Membuka folder.
   - Membuat workspace.

2. **Coding Workspace**
   - File explorer.
   - AI chat.
   - Code editor.
   - Terminal.
   - Diff viewer.
   - Status agent dan proses tool.

3. **Provider Center**
   - Daftar provider.
   - Konfigurasi endpoint dan autentikasi.
   - Tes koneksi.
   - Daftar model dan capability.

4. **Agent Studio**
   - Membuat profil agent.
   - Mengatur system prompt.
   - Memilih model dan tools.
   - Mengatur permission.

5. **Sessions**
   - Riwayat percakapan.
   - Pencarian sesi.
   - Melanjutkan sesi sebelumnya.

6. **Usage & Settings**
   - Penggunaan token dan estimasi biaya.
   - Shortcut keyboard.
   - Tema.
   - Privasi dan retensi data.
   - Konfigurasi aplikasi.

### 9.2 Interaksi utama

- Command palette.
- Shortcut untuk membuka file, terminal, dan pencarian.
- Drag-and-drop file ke chat.
- Klik referensi file dari jawaban agent untuk membuka editor.
- Klik perubahan untuk melihat diff.
- Tombol stop untuk membatalkan generasi atau proses.
- Status tool real-time.
- Draft instruksi dipertahankan ketika aplikasi ditutup atau proses terputus, sejauh memungkinkan.

## 10. Permission dan Keamanan

Agent dapat mengakses sumber daya lokal sehingga keamanan merupakan kebutuhan inti.

| Aksi | Kebijakan awal |
|---|---|
| Membaca file dalam workspace | Diizinkan, dengan pengecualian file sensitif |
| Mencari file dan teks | Diizinkan dalam cakupan workspace |
| Mengubah file | Memerlukan persetujuan pada mode aman |
| Menjalankan command terminal | Memerlukan persetujuan |
| Mengakses file di luar workspace | Ditolak atau meminta izin eksplisit |
| Menghapus file | Memerlukan persetujuan |
| `git push` atau operasi remote | Memerlukan persetujuan |
| Membaca `.env` dan secret | Dibatasi dan diberi peringatan |
| Mengirim konteks ke provider cloud | Transparan dan dapat dikontrol pengguna |

### Ketentuan teknis

- Simpan API key menggunakan OS keychain/credential vault yang sesuai.
- Jangan menyimpan secret dalam log, SQLite biasa, atau file konfigurasi proyek.
- Validasi path dan batasi akses filesystem pada workspace.
- Tangani symlink dan path traversal.
- Gunakan process manager yang dapat menghentikan proses turunan.
- Terapkan timeout, batas output, dan pembatalan proses.
- Tampilkan command yang akan dijalankan saat persetujuan diperlukan.
- Perlakukan instruksi dari file proyek dan hasil tool sebagai input tidak tepercaya.
- Sediakan checkpoint atau patch backup agar perubahan dapat dipulihkan.
- Persetujuan pengguna bukan pengganti isolasi proses dan filesystem.

## 11. Struktur Data Lokal

SQLite menyimpan metadata aplikasi. Source code proyek tetap berada di folder aslinya.

| Tabel | Kolom penting |
|---|---|
| `workspaces` | `id`, `name`, `root_path`, `last_opened_at` |
| `providers` | `id`, `name`, `type`, `base_url`, `enabled` |
| `models` | `id`, `provider_id`, `model_name`, `capabilities` |
| `agent_profiles` | `id`, `name`, `system_prompt`, `permissions` |
| `sessions` | `id`, `workspace_id`, `title`, `agent_id`, `model_id` |
| `messages` | `id`, `session_id`, `role`, `content`, `created_at` |
| `tool_runs` | `id`, `message_id`, `tool_name`, `status`, `result_summary` |
| `approvals` | `id`, `tool_run_id`, `decision`, `created_at` |
| `usage_records` | `id`, `session_id`, `provider_id`, `input_tokens`, `output_tokens`, `cost_estimate` |

### Aturan penyimpanan

- API key tidak disimpan langsung di tabel `providers`.
- Simpan referensi credential aman, bukan nilai secret.
- Tambahkan database migration.
- Sediakan retensi, ekspor, dan penghapusan riwayat.
- Informasikan data apa yang dikirim ke provider.
- Hindari menyimpan isi file atau secret ke log diagnostik tanpa persetujuan.

## 12. Struktur Repository

```text
jenderalcode-desktop/
├── apps/
│   └── desktop/                 # Tauri application
├── crates/
│   ├── agent-runtime/           # Agent loop and orchestration
│   ├── provider-core/           # Provider trait and registry
│   ├── provider-openai/         # OpenAI-compatible APIs
│   ├── provider-anthropic/      # Anthropic adapter
│   ├── provider-google/         # Google adapter
│   ├── agent-cli/               # External CLI adapters
│   ├── tool-filesystem/         # Read, write, patch, search
│   ├── tool-terminal/           # PTY and process manager
│   ├── tool-git/                # Git operations
│   ├── permissions/             # Policy engine
│   ├── storage/                 # SQLite and migrations
│   └── common/                  # Shared types and events
├── frontend/
│   ├── src/
│   │   ├── components/
│   │   ├── features/
│   │   │   ├── workspace/
│   │   │   ├── chat/
│   │   │   ├── providers/
│   │   │   ├── agents/
│   │   │   ├── terminal/
│   │   │   └── diff/
│   │   └── stores/
│   └── package.json
├── docs/
├── tests/
└── README.md
```

Struktur tersebut adalah rancangan awal. Nama crate, batas modul, dan susunan workspace dapat disesuaikan setelah proof of concept.

### 12.1 Kontrak provider

Setiap adapter mengimplementasikan kontrak internal seragam. Contoh konseptual:

```rust
#[async_trait::async_trait]
pub trait AiProvider: Send + Sync {
    async fn list_models(
        &self,
    ) -> anyhow::Result<Vec<ModelInfo>>;

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<ModelStream>;
}
```

`ModelInfo`, `ModelRequest`, dan `ModelStream` merupakan tipe internal yang perlu didefinisikan. Kontrak ini bukan kode lengkap yang siap dikompilasi.

Runtime bertanggung jawab untuk:
- Menormalisasi respons provider menjadi event internal.
- Mengelola tool call dan mengirim hasil tool kembali ke model.
- Mengontrol iterasi, timeout, dan pembatalan.
- Menangani perbedaan streaming dan tool calling.
- Mengirim event ke frontend melalui Tauri.
- Memastikan setiap tool melewati permission engine.

## 13. Strategi Pengujian

| Area | Pengujian |
|---|---|
| Provider adapter | Unit test, mock HTTP server, contract test |
| Streaming | Chunk parsial, koneksi terputus, pembatalan |
| Filesystem | Path traversal, symlink, file besar, permission |
| Terminal | Timeout, sinyal penghentian, output besar, proses anak |
| Agent runtime | Tool call berulang, batas iterasi, error recovery |
| Diff | Patch valid, konflik, perubahan eksternal, rollback |
| Storage | Migrasi, integritas data, pemulihan sesi |
| Desktop | Smoke test, instalasi, update, pembukaan workspace pada semua OS |

Gunakan mock provider untuk pengujian otomatis agar tes tidak selalu membutuhkan API berbayar.

## 14. Tahapan Pengembangan

Estimasi awal untuk satu developer berpengalaman atau tim kecil. Durasi bergantung pada kompleksitas integrasi agent dan tingkat polish UI.

| Tahap | Estimasi | Hasil |
|---|---:|---|
| M1 — Desktop foundation | 2 minggu | Setup Tauri, UI workspace, folder picker, SQLite, credential storage |
| M2 — Provider + chat | 2–3 minggu | Provider registry, streaming chat, model selector, file context, riwayat sesi |
| M3 — Coding agent | 3 minggu | Tool loop, filesystem, terminal, permission, patch editing, diff viewer |
| M4 — Stabilitas dan distribusi | 2–3 minggu | Pengujian lintas OS, recovery, satu CLI adapter, installer, dokumentasi |

**Estimasi total MVP: 9–11 minggu.** Multi-agent orchestration, sistem plugin penuh, dan integrasi agent yang kompleks sebaiknya dilakukan setelah MVP stabil.

## 15. Kriteria Penerimaan MVP

- [ ] Aplikasi membuka folder lokal dan mengingat workspace setelah restart.
- [ ] Pengguna dapat mengonfigurasi minimal dua provider berbeda.
- [ ] Pengguna dapat memilih model untuk setiap sesi.
- [ ] Respons AI tampil bertahap dan dapat dibatalkan.
- [ ] Agent dapat membaca, mencari, mengedit file, dan menjalankan tes.
- [ ] Pengguna dapat meninjau diff sebelum menerima perubahan.
- [ ] Operasi terminal dan tindakan sensitif mengikuti kebijakan izin.
- [ ] Riwayat percakapan tetap tersedia setelah aplikasi ditutup.
- [ ] API key tidak disimpan sebagai teks biasa dalam database.
- [ ] Build dapat dijalankan dan diuji pada macOS, Windows, dan Linux.

## 16. Risiko dan Mitigasi

| Risiko | Mitigasi |
|---|---|
| Format API berbeda-beda | Adapter per provider dan contract tests |
| Agent salah mengubah file | Patch-based editing, diff review, checkpoint |
| Shell command berbahaya | Permission engine, isolasi, proses yang bisa dihentikan |
| Biaya model meningkat | Informasi token, estimasi biaya, batas penggunaan |
| Context window tidak cukup | Context selection, ringkasan sesi, pengelolaan token |
| Model tidak mendukung tool calling | Capability detection dan fallback yang jelas |
| API key bocor | OS credential storage dan redaksi log |
| Perubahan protokol CLI | Adapter terpisah, version checks, integration tests |

## 17. Keputusan Produk

1. **Rust menjadi backend utama**, bukan hanya pembungkus UI.
2. **Provider API dan external coding agent memiliki adapter berbeda.**
3. **Local-first:** aplikasi tidak memerlukan backend cloud milik JenderalCode untuk fitur inti.
4. **Approval-based:** tindakan sensitif dapat ditinjau dan dikontrol pengguna.
5. **Mulai dengan sedikit integrasi yang stabil**, kemudian tambah provider dan agent secara bertahap.
6. **Utamakan pengalaman coding inti** sebelum membangun multi-agent orchestration atau marketplace.

## 18. Referensi Teknis

- Tauri 2: https://v2.tauri.app/
- OpenCode provider documentation: https://docs.opencode.ai/docs/providers/
- OpenCode agents documentation: https://opencode.ai/docs/agents

Dokumentasi dan kompatibilitas provider perlu diperiksa kembali ketika implementasi dimulai.

---

## Ringkasan

**JenderalCode Desktop** adalah desktop AI coding workspace berbasis Rust yang menyatukan berbagai provider AI, model lokal, dan coding agent eksternal. Fokus MVP adalah workspace lokal, provider management, streaming chat, agent tools, terminal, diff review, session history, dan permission system.

Strategi paling realistis adalah membangun runtime agent yang independen, memulai dari direct provider API, dan menambahkan external CLI adapter setelah alur utama stabil.
