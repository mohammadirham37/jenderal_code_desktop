# Arsitektur JenderalCode Desktop

Dokumen ini memetakan implementasi aktual ke rancangan pada PRD (§5, §12).

## Alur data satu giliran agent

```
UI (React)
  │  invoke("send_message", { session_id, content, attachments })
  ▼
Tauri command (apps/desktop/src/commands.rs)
  │
  ▼
AgentRuntime::send_message  ──────────────►  spawn(tokio task per giliran)
  crates/agent-runtime/src/lib.rs                     │
                                                      ▼
                              build_system_prompt()   │  context.rs: profil + tree +
                                                      │  git status + instruksi proyek
                                                      ▼
                              provider.stream(ModelRequest)   (adapter per kind)
                                                      │  openai | anthropic | google | mock
                                                      ▼
                              loop { StreamEvent }    │  TextDelta → emit chat://delta
                                                      │  ToolCall*  → akumulasi
                                                      │  Done       → persist pesan assistant
                                                      ▼
                              untuk tiap tool call:
                                1. executor::classify → permissions::Action
                                2. PolicyEngine::check
                                     Allowed     → eksekusi
                                     Denied      → hasil "blocked"
                                     NeedsApproval → ApprovalRecord + emit
                                                     chat://approval → UI menampilkan kartu
                                                     invoke("decide_approval") → oneshot → lanjut
                                3. executor::execute
                                     - checkpoint snapshot (tool-filesystem)
                                     - read/write/edit/search | terminal_run | git_*
                                4. persist Tool message + emit chat://tool / chat://message
                                                      ▼
                              ulangi hingga tanpa tool call / limit iterasi / cancel
```

Setiap event dilewatkan `EventSink` (trait di agent-runtime) yang diimplementasikan
`TauriEventSink` di `apps/desktop/src/sink.rs` menjadi emit Tauri.

## Batas keamanan (choke points)

| Kekhawatiran | Penanganan | Lokasi |
|---|---|---|
| Path traversal / symlink escape | `Workspace::resolve` — canonicalize + containment; jalur baru dinormalisasi leksikal | `tool-filesystem/src/lib.rs` |
| Akses di luar workspace | Ditolak untuk tulis; baca perlu persetujuan eksplisit | `permissions/src/lib.rs` |
| File sensitif (.env, key) | Selalu butuh persetujuan + peringatan | `permissions::is_sensitive_path` |
| Perintah destruktif | Tokenizer per segmen + deny-list mutlak (rm -rf /, mkfs, …) | `permissions::assess_command`, `is_catastrophic_command` |
| git push/pull/fetch/remote, sudo, curl\|sh | Minta persetujuan di Safe/Balanced; Yolo (Full access) selalu mengizinkan | `permissions::is_git_remote_command` |
| Proses liar | Timeout + cap output + kill process group | `tool-terminal/src/lib.rs` |
| Edit tak terpulihkan | Checkpoint sebelum tiap tulis/hapus; `restore_checkpoints` | `tool-filesystem::CheckpointStore` |
| Kebocoran API key | File 0600 (tulis atomik) atau OS keychain; DB hanya menyimpan referensi; env rahasia dihapus dari proses anak | `provider-core/src/credentials.rs`, `tool-terminal::is_secret_env_name` |
| Exfiltrasi via webview | CSP ketat + navigation lock; markdown memblokir gambar & navigasi link | `tauri.conf.json`, `apps/desktop/src/main.rs`, `ChatPanel.tsx` |
| Instruksi proyek (AGENTS.md) | Ditandai UNTRUSTED dalam system prompt | `agent-runtime/src/context.rs` |

## Kontrak provider

`AiProvider` (provider-core) — `list_models()` dan `stream(ModelRequest) -> BoxStream<StreamEvent>`.
Tipe internal (`ChatMessage`, `ToolSpec`, `StreamEvent`) didefinisikan di `jcode-common`.
Setiap adapter bertanggung jawab untuk:

- konversi pesan historis (termasuk tool call/result) ke format wire-nya,
- deklarasi tools dalam format native (function tools / input_schema / functionDeclarations),
- parsing SSE menjadi `StreamEvent` (TextDelta, ToolCallDelta/Complete, Usage, Done),
- penandaan kapabilitas tidak diketahui sebagai `None` (tidak menebak, PRD §6.3).

Adapter OpenAI-compatible menjangkau OpenAI/DeepSeek/Mistral/xAI/Groq/Ollama/LM Studio/gateway
karena protokolnya identik; preset URL ada di `provider-core::provider_presets()`.

## Penyimpanan

SQLite bundled via rusqlite; migrasi tertanam di `crates/storage/migrations/`.
Tabel sesuai PRD §11. Satu koneksi di balik mutex — sesuai skala aplikasi desktop.
Retensi, ekspor JSON, dan hapus-total tersedia untuk kepatuhan privasi (PRD §11).

## Terminal

Dua jalur berbeda:

1. **Interaktif** (untuk manusia): `apps/desktop/src/terminal.rs` membuka PTY
   (portable-pty), shell interaktif per tab, output diteruskan ke xterm.js via
   `terminal://data`.
2. **Eksekusi tool** (untuk agent): `tool-terminal::run_exec` — non-interaktif,
   dengan `timeout_ms`, `max_output_bytes`, dan kill process group; dibatasi
   permission engine.

## File watching

`apps/desktop/src/watcher.rs` memakai crate `notify` (recursive) dengan debounce
300 ms; event dikirim sebagai `fs://changed` untuk menyegarkan file tree dan
editor (file yang sedang diedit tidak ditimpa).

## Mode agent

| Mode | Tools | Policy efektif |
|---|---|---|
| Plan | read-only (tanpa terminal) | semua tulis ditolak di runtime |
| Build | semua | dari profil (default Safe) |
| Review | read-only + terminal_run (uji) | tulis ditolak; perintah tetap lewat policy |
| Custom | sesuai profil | sesuai profil |

Penegakan ganda: tools tidak dikirim ke model (specs) **dan** ditolak saat
eksekusi bila dipanggil (defense in depth).

## Debugging tips

- Log event: dengarkan topik `chat://*`, `terminal://*`, `fs://changed` dari
  frontend devtools.
- Data aplikasi: `Settings → Privacy` menampilkan path data (SQLite, checkpoint,
  fallback kredensial).
- Mock provider dapat dipakai untuk mereproduksi alur lengkap tanpa jaringan.
