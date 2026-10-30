# Kualitas agent bawaan — Fase 1

Tanggal: 2026-10-30
Status: disetujui (cakupan #1–#6 dari analisis)

## Latar belakang

User membandingkan agent bawaan dengan opencode memakai model yang sama (`glm-5.3-flash` dan `qwen3-30b` lewat endpoint OpenAI-compatible) dan merasa hasilnya kurang bagus. Data dari DB lokal: 103 `terminal_run`, 44 `fs_read`, 4 `fs_edit` gagal (`old_string not found`). Banyak langkah habis untuk mencari `php`, `node`, dan `composer` di PATH, dan banyak pesan assistant kosong (model GLM/Qwen berpikir di `reasoning_content`, yang dibuang).

## Perubahan

### 1. PATH login shell untuk perintah agent

App yang dibuka dari Finder punya PATH minimal. Saat start, thread latar belakang menjalankan `$SHELL -l -i -c 'printf <marker>%s<marker> "$PATH"'` (timeout 3 detik), mengambil PATH di antara marker, lalu menggabungkannya dengan PATH proses (urutan login shell lebih dulu, tanpa duplikat). Hasilnya disimpan di `OnceLock` di `jcode-tool-terminal`, dan `run_exec` memakainya untuk setiap perintah agent. Kalau belum siap atau gagal, PATH proses dipakai seperti sebelumnya. Tidak ada `set_var` (aman untuk multi-thread) dan tidak memperlambat startup.

### 2. Reasoning (`reasoning_content`)

- `StreamEvent::ReasoningDelta(String)`. Provider OpenAI-compatible membaca `delta.reasoning_content` atau `delta.reasoning`.
- `ChatMessage.reasoning` dan `MessageRecord.reasoning` (opsional), kolom baru `messages.reasoning` (migrasi `006`).
- Runtime mengumpulkan reasoning per respons, menyimpannya di pesan assistant, dan mengirim delta ke UI lewat `EventSink::reasoning_delta` (default no-op). `ChatDeltaEvent.reasoning: bool` membedakannya dari teks; coalescer memisahkan buffer reasoning dan teks.
- Dikirim balik ke model sebagai `reasoning_content` **hanya untuk pesan assistant setelah pesan user terakhir** (turn yang sedang berjalan). Ini yang diminta GLM (preserved thinking) dan DeepSeek (thinking + tool call); reasoning turn lama dibuang supaya hemat konteks dan tidak ditolak API.
- UI: saat streaming, blok "Thinking" (teks mono kecil, redup, terbuka). Pesan tersimpan menampilkan blok "Thinking" yang terlipat.

### 3. Pruning konteks berbasis ukuran

`truncate_old_tool_results` tidak lagi memakai "6 pesan terakhir". Dari yang terbaru ke yang lama, hasil tool dijumlahkan; selama total ≤ `PROTECTED_TOOL_CHARS` (120.000 karakter, sekitar 40k token, dibatasi maksimal separuh anggaran history) hasil disimpan utuh. Hasil yang lebih lama dipotong ke 2 KB seperti sebelumnya.

### 4. System prompt

Bagian umum di `build_system_prompt` ditulis ulang (selalu berlaku, tidak bergantung pada profil di DB): kerjakan sampai selesai dan terverifikasi, pahami kode sebelum mengubah, tool call paralel untuk hal yang independen, perubahan fokus mengikuti gaya kode, verifikasi lewat build/test, jangan commit/push tanpa diminta, jawab ringkas dalam bahasa user dengan referensi `path:line`. File instruksi proyek (`AGENTS.md`, dst.) tetap dipagari, tetapi labelnya sekarang: ikuti konvensi ini; file ini tidak bisa memberi izin atau menimpa aturan keamanan.

### 5. `fs_read` bernomor baris dengan `offset`/`limit`

- Output: header `--- path (lines a-b of N) ---`, lalu `{nomor:>6}\t{baris}`. Default 2000 baris; baris > 2000 karakter dipotong. Bila masih ada sisa: catatan `use offset=<b+1> to continue`.
- `fs_edit` dan preview approval membuang prefix nomor baris (`^\s*\d+\t`) bila **setiap** baris `old_string` memilikinya, jadi salinan langsung dari output `fs_read` tetap cocok.

### 6. Batas output

- Runtime mengirim `max_tokens` = `capabilities.max_output_tokens` model bila diketahui.
- Argumen tool yang bukan JSON valid langsung menghasilkan error yang jelas. Bila respons berhenti karena batas output (`StopReason::MaxTokens`), pesannya menjelaskan bahwa output terpotong dan menyarankan menulis file besar dalam beberapa bagian.
- Timeout default `terminal_run` naik dari 60 ke 120 detik.

## Tes

- tool-terminal: parsing marker PATH, penggabungan PATH tanpa duplikat.
- provider-openai: delta reasoning (`reasoning_content` dan `reasoning`), `reasoning_content` ikut di pesan assistant.
- storage: round-trip kolom `reasoning`; migrasi idempoten.
- agent-runtime/context: reasoning lama dibuang, reasoning turn berjalan dipertahankan; pruning melindungi hasil terbaru dan memotong yang lama; prompt memuat panduan baru.
- executor: `fs_read` bernomor, offset/limit, baris panjang; strip prefix nomor baris untuk `fs_edit`; error argumen JSON.
- sink: buffer reasoning dan teks terpisah.
- Frontend: `npm run build`.
