/** xterm.js instances that live outside React. A terminal is created before
 *  its PTY (so early output is not lost) and its DOM element moves between
 *  hosts, so scrollback survives remounts (page or layout switches). */
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { api, events } from "@/lib/ipc";

interface Entry {
  term: XTerm;
  fit: FitAddon;
  el: HTMLDivElement;
  opened: boolean;
}

const entries = new Map<string, Entry>();
let listening = false;

const THEME = {
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

function listen() {
  if (listening) return;
  listening = true;
  void events.terminalData((e) => entries.get(e.terminal_id)?.term.write(e.data));
  void events.terminalExit((e) =>
    entries.get(e.terminal_id)?.term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n"),
  );
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
      theme: THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.onData((data) => void api.terminalWrite(id, data).catch(() => {}));
    term.onResize(({ cols, rows }) => void api.terminalResize(id, cols, rows).catch(() => {}));
    const el = document.createElement("div");
    el.className = "h-full w-full";
    entry = { term, fit, el, opened: false };
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
  if (entry?.opened && entry.el.clientWidth > 0 && entry.el.clientHeight > 0) entry.fit.fit();
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
