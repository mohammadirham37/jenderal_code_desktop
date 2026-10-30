# Tab terminal & agent CLI di kolom tengah (Chat layout)

Tanggal: 2026-10-30
Status: disetujui

## Tujuan

Kolom tengah layout `"chat"` (sekarang hanya `ChatPanel variant="full"`) mendapat tab strip seperti aplikasi Orca. Selain tab **Chat** (agent bawaan JenderalCode), user bisa membuka tab terminal biasa atau tab yang langsung menjalankan agent CLI pihak ketiga (Claude Code, Codex, OpenCode, Gemini, Goose, Aider, atau command custom).

## Di luar cakupan

- Layout `"code"` (`WorkspacePage`) tidak berubah.
- Tab tidak dipulihkan setelah app restart.
- Tab Terminal di context panel kanan tetap ada dan tidak berubah perilakunya.
- Tidak ada integrasi antara agent CLI dan sistem approval/checkpoint JenderalCode; agent CLI berjalan sebagai proses terminal biasa milik user.

## Backend (Rust)

### `TerminalManager::create`

Signature menjadi `create(app, id, cwd, cols, rows, command: Option<&str>)`.

- `None`: perilaku sekarang (login + interactive shell).
- `Some(cmd)`: jalankan `$SHELL -l -i -c <cmd>` di PTY (Windows: `pwsh -Command <cmd>` / `cmd.exe /C <cmd>`). Login shell dipakai agar PATH dari profile user terbaca; app yang dibuka dari Finder tidak mewarisi PATH terminal.
- Env tetap di-scrub dengan `scrubbed_env_keys()` dan `TERM`/`COLORTERM` tetap di-set. API key milik app tidak diteruskan; agent CLI memakai auth miliknya sendiri.
- Command Tauri `terminal_create` mendapat argumen opsional `command: Option<String>`.

`command` hanya berasal dari settings user (UI), tidak pernah dari output model, jadi tidak lewat `PolicyEngine`.

### Deteksi agent terpasang

Command Tauri baru `detect_agent_clis(commands: Vec<String>) -> Vec<bool>` (async, body di `spawn_blocking`):

- Ambil kata pertama setiap command (nama program).
- Satu kali panggilan `$SHELL -l -c 'command -v a >/dev/null 2>&1 && echo 1 || echo 0; …'` dengan nama program di-quote aman; hasil dipetakan berurutan. Nama program yang mengandung karakter di luar `[A-Za-z0-9._+-]` langsung dianggap `false` tanpa masuk shell.
- Timeout 5 detik; gagal/timeout → semua `false`.
- Windows: pakai `which()` yang sudah ada.

Logika parsing/quoting ada di fungsi murni agar bisa dites tanpa spawn.

### Settings

`AppSettings.agent_clis: Vec<AgentCli>` dengan `#[serde(default = "default_agent_clis")]`:

```rust
pub struct AgentCli {
    pub id: String,      // stabil, mis. "claude"
    pub name: String,    // label menu
    pub command: String, // mis. "claude"
    pub enabled: bool,   // tampil di menu +
}
```

Preset default: Claude (`claude`), Codex (`codex`), OpenCode (`opencode`), Gemini (`gemini`), Goose (`goose session`), Aider (`aider`). Settings lama tanpa field ini mendapat preset lewat serde default.

## Frontend

### `XtermView`

Logika xterm dari `TerminalPanel` (pembuatan instance, tema, fit, resize observer, font size, event data/exit) diekstrak ke komponen `features/terminal/XtermView.tsx` yang menerima `terminalId` dan `visible`. `TerminalPanel` dan tab tengah sama-sama memakainya.

Saat proses exit, `XtermView` menulis `[process exited]` dan memanggil `onExit`; tab tengah lalu menampilkan tombol **Restart** yang membuat ulang PTY dengan id dan command yang sama.

### Store `useCenterTabs`

Zustand store: `{ tabs: CenterTab[], activeId: "chat" | string }` dengan `CenterTab = { id, title, command?: string, agentId?: string, exited: boolean }`.

- `openTerminal()` / `openAgent(agent)`: buat id, panggil `terminalCreate(id, 80, 24, command)`, tambah tab, aktifkan.
- `close(id)`: `terminalKill` lalu hapus tab; tab aktif pindah ke tetangga atau Chat.
- `reset()`: kill semua PTY dan kosongkan tab. Dipanggil saat workspace berganti dan saat `ChatLayout` unmount.
- Judul tab: nama agent, atau `zsh 2`, dst. untuk terminal biasa.

### `ChatLayout` – `CenterTabs`

- Tab strip di atas kolom tengah: **Chat** (selalu ada, tidak bisa ditutup), lalu tab terminal/agent dengan tombol tutup, lalu tombol **+**.
- Isi: `ChatPanel` dan setiap `XtermView` tetap ter-mount; yang tidak aktif disembunyikan (`hidden`), jadi PTY dan state chat bertahan saat pindah tab atau ganti sesi chat.
- Tombol toggle context panel tetap di pojok kanan atas.

### Menu +

Popover dengan:

1. **New Terminal** (⌘T)
2. Pemisah, lalu agent dengan `enabled = true`. Saat menu dibuka, `detectAgentClis` dipanggil sekali per workspace (hasil di-cache di store). Agent yang tidak terdeteksi tampil redup dengan label `not installed`, tetapi tetap bisa diklik (deteksi bisa salah, mis. alias).
3. Pemisah, lalu **Agent settings…** yang membuka Settings ke section Agent CLIs.

Ikon: lucide (`Terminal`, `Bot`), tanpa logo vendor. Gaya mengikuti visual system (`neutral-*`, `accent-*`, `font-mono`, radius kecil).

⌘T membuka terminal baru saat layout `"chat"` aktif dan fokus tidak berada di input teks.

### Settings page

Section "Agent CLIs": daftar baris (nama, command, toggle enabled, hapus) dan tombol "Add agent". Disimpan lewat mekanisme `save_settings` yang sudah ada.

### `ipc.ts`

Tambah tipe `AgentCli`, field `agent_clis` di `AppSettings`, argumen `command?` di `terminalCreate`, wrapper `detectAgentClis`.

## Error handling

- `terminalCreate` gagal: tab tetap dibuat dengan pesan error merah di area terminal dan tombol Retry.
- Deteksi gagal: semua agent dianggap tidak terdeteksi (tetap bisa diklik).

## Tes

- Rust (`apps/desktop`): fungsi murni builder skrip deteksi (quoting, penolakan nama tidak valid, pemetaan output); serde default `agent_clis` pada JSON settings lama.
- Rust: helper yang membangun `CommandBuilder` untuk `Some(cmd)` menghasilkan argumen `-l -i -c cmd`.
- Frontend: `npm run build` (tsc + vite).
- Manual: buka tab Claude dan zsh, ganti sesi chat, pastikan PTY tetap hidup; exit agent lalu Restart; ganti workspace, pastikan semua PTY mati.
