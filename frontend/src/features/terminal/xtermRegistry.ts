/** xterm.js instances that live outside React. A terminal is created before
 *  its PTY (so early output is not lost) and its DOM element moves between
 *  hosts, so scrollback survives remounts (page or layout switches). */
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, events, type TerminalDataEvent } from "@/lib/ipc";

interface Entry {
  term: XTerm;
  fit: FitAddon;
  el: HTMLDivElement;
  opened: boolean;
  /** Output held back while joining a running terminal (`joinXterm`). */
  sync: TerminalDataEvent[] | null;
  /** PTY size owned elsewhere (a phone): show it at this size, never fit
   *  or resize the PTY from here. */
  fixed: { cols: number; rows: number } | null;
}

const entries = new Map<string, Entry>();
let listening: Promise<unknown> | null = null;

/** Dark palette: CLI colors are shown as the CLI sends them. */
const DARK = {
  background: "#0c0f0e",
  foreground: "#d6ddda",
  cursor: "#2fc3a5",
  cursorAccent: "#0c0f0e",
  selectionBackground: "#13a68b55",
  black: "#151a18",
  brightBlack: "#67746f",
  green: "#2fc3a5",
  brightGreen: "#62dbbf",
  cyan: "#56b6c2",
  brightCyan: "#7fd3dc",
};

/** Light palette (app neutrals/accent). "white" is dark grey: CLIs print
 *  plain text in white, which would vanish on a light background. */
const LIGHT = {
  background: "#f5f7f6",
  foreground: "#1c2220",
  cursor: "#0b8672",
  cursorAccent: "#f5f7f6",
  selectionBackground: "#13a68b40",
  black: "#1c2220",
  red: "#c2412d",
  green: "#0b8672",
  yellow: "#9a6700",
  blue: "#2a62b8",
  magenta: "#8e44ad",
  cyan: "#0e7490",
  white: "#67746f",
  brightBlack: "#4e5954",
  brightRed: "#d4513d",
  brightGreen: "#0d6b5c",
  brightYellow: "#7c5800",
  brightBlue: "#1d4f99",
  brightMagenta: "#7a3b96",
  brightCyan: "#0b5f75",
  brightWhite: "#252d2a",
};

const isDark = () => document.documentElement.classList.contains("dark");

/** Theme options for the current app theme. In light mode xterm raises the
 *  contrast of colors a CLI picked for dark terminals (e.g. truecolor greys). */
function themeOptions() {
  return isDark() ? { theme: DARK, minimumContrastRatio: 1 } : { theme: LIGHT, minimumContrastRatio: 4.5 };
}

// Follow the app theme (the `dark` class on <html>): every terminal, including
// ones parked with another project, switches at once.
let lastDark = isDark();
new MutationObserver(() => {
  const dark = isDark();
  if (dark === lastDark) return;
  lastDark = dark;
  const opts = themeOptions();
  for (const { term } of entries.values()) {
    term.options.theme = opts.theme;
    term.options.minimumContrastRatio = opts.minimumContrastRatio;
  }
}).observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });

/** The app's mono stack as a literal string: xterm measures with canvas,
 *  where `var(--font-mono)` is invalid. */
function monoFamily(): string {
  return (
    getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() ||
    '"IBM Plex Mono", ui-monospace, monospace'
  );
}

/** Web fonts load lazily; measuring before they arrive sizes cells for the
 *  fallback font. */
const fontsReady: Promise<unknown> = Promise.all(
  ["400", "600"].map((w) => document.fonts.load(`${w} 12px "IBM Plex Mono"`)),
).catch(() => {});

function listen(): Promise<unknown> {
  listening ??= Promise.all([
    events.terminalData((e) => {
      const entry = entries.get(e.terminal_id);
      if (entry?.sync) entry.sync.push(e);
      else entry?.term.write(e.data);
    }),
    events.terminalExit((e) =>
      entries.get(e.terminal_id)?.term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n"),
    ),
    events.terminalResized((e) => {
      const entry = entries.get(e.terminal_id);
      if (!entry?.fixed) return;
      entry.fixed = { cols: e.cols, rows: e.rows };
      entry.term.resize(e.cols, e.rows);
    }),
  ]);
  return listening;
}

export function newTerminalId(): string {
  return `term-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

/** Get or create the xterm for `id`. Call before `api.terminalCreate`. */
export function ensureXterm(id: string, fontSize: number): Entry {
  listen();
  let entry = entries.get(id);
  if (!entry) {
    const term = new XTerm({
      fontSize,
      fontFamily: monoFamily(),
      fontWeight: "400",
      fontWeightBold: "600",
      lineHeight: 1.2,
      cursorBlink: true,
      ...themeOptions(),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.onData((data) => void api.terminalWrite(id, data).catch(() => {}));
    term.onResize(({ cols, rows }) => {
      if (entries.get(id)?.fixed) return;
      void api.terminalResize(id, cols, rows).catch(() => {});
    });
    const el = document.createElement("div");
    el.className = "h-full w-full";
    entry = { term, fit, el, opened: false, sync: null, fixed: null };
    entries.set(id, entry);
  }
  return entry;
}

/** Move the terminal into `host` (opening it on first attach) and size it. */
export function attachXterm(id: string, host: HTMLElement, fontSize: number) {
  const entry = ensureXterm(id, fontSize);
  if (entry.el.parentElement !== host) host.appendChild(entry.el);
  entry.term.options.fontSize = fontSize;
  if (entry.opened) {
    fitXterm(id);
    return;
  }
  void fontsReady.then(() => {
    // Skip if the tab was closed (or already opened) while fonts loaded.
    if (entry.opened || entries.get(id) !== entry) return;
    entry.term.open(entry.el);
    entry.opened = true;
    fitXterm(id);
    // The view's focus-on-activate ran before the terminal existed.
    if (entry.el.clientWidth > 0) entry.term.focus();
  });
}

/** Refit to the host; skipped while hidden (zero size). */
export function fitXterm(id: string) {
  const entry = entries.get(id);
  if (entry?.fixed) {
    entry.term.resize(entry.fixed.cols, entry.fixed.rows);
    return;
  }
  if (entry?.opened && entry.el.clientWidth > 0 && entry.el.clientHeight > 0) entry.fit.fit();
}

/** Show a terminal that was started elsewhere (by a phone): its recent
 *  output, then live output, without gaps or repeats (`seq`). It keeps the
 *  PTY's size, which the phone owns, until `unfixXterm`. */
export async function joinXterm(id: string, fontSize: number) {
  const entry = ensureXterm(id, fontSize);
  entry.sync = [];
  entry.fixed = { cols: entry.term.cols, rows: entry.term.rows };
  let after = 0;
  try {
    await listen();
    const snap = await api.terminalSnapshot(id);
    entry.fixed = { cols: snap.cols, rows: snap.rows };
    entry.term.resize(snap.cols, snap.rows);
    entry.term.write(snap.data);
    after = snap.seq;
  } finally {
    // On failure (the terminal already exited) show whatever arrived.
    for (const e of entry.sync ?? []) if (e.seq > after) entry.term.write(e.data);
    entry.sync = null;
  }
}

/** Let the desktop size the terminal again (after a restart here). */
export function unfixXterm(id: string) {
  const entry = entries.get(id);
  if (!entry) return;
  entry.fixed = null;
  fitXterm(id);
}

export function focusXterm(id: string) {
  entries.get(id)?.term.focus();
}

export function writeXterm(id: string, text: string) {
  entries.get(id)?.term.write(text);
}

export function disposeXterm(id: string) {
  const entry = entries.get(id);
  if (!entry) return;
  entry.term.dispose();
  entry.el.remove();
  entries.delete(id);
}
