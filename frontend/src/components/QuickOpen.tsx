/** Quick Open (⌘P on macOS, Ctrl+P elsewhere): fuzzy file search over the
 *  open project via `fs_find_files` (gitignore-aware, no symlinks or
 *  secret-like files), opening the pick in the editor. */
import { useEffect, useRef, useState } from "react";
import { FileText } from "lucide-react";
import { useApp } from "@/stores/app";
import { useEditor } from "@/stores/editor";
import { api } from "@/lib/ipc";

const isMac = navigator.platform.toUpperCase().includes("MAC");

/** Open file tabs first (most recent last in the tab list → reversed), then
 *  search results, without duplicates. */
function mergeResults(open: string[], found: string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  const openMatches = q ? open.filter((p) => p.toLowerCase().includes(q)) : open;
  const seen = new Set(openMatches);
  return [...openMatches, ...found.filter((p) => !seen.has(p))];
}

export function QuickOpen() {
  const isOpen = useApp((s) => s.quickOpen);
  const setQuickOpen = useApp((s) => s.setQuickOpen);
  const workspace = useApp((s) => s.workspace);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<string[]>([]);
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const combo = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
      if (!combo || e.altKey || e.shiftKey || e.code !== "KeyP") return;
      // Ctrl+P is "previous command" in shells and agent CLIs; leave it to
      // the terminal outside macOS.
      if (!isMac && (e.target as HTMLElement | null)?.closest(".xterm")) return;
      e.preventDefault();
      const app = useApp.getState();
      if (!app.workspace) return;
      app.setPaletteOpen(false);
      app.setQuickOpen(!app.quickOpen);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Close when the project goes away.
  useEffect(() => {
    if (!workspace) setQuickOpen(false);
  }, [workspace, setQuickOpen]);

  useEffect(() => {
    if (!isOpen) return;
    let stale = false;
    const t = setTimeout(
      () => {
        api
          .fsFindFiles(query)
          .then((found) => {
            if (stale) return;
            const open = useEditor
              .getState()
              .tabs.flatMap((tab) => (tab.kind === "file" ? [tab.path] : []))
              .reverse();
            setResults(mergeResults(open, found, query));
          })
          .catch(() => !stale && setResults([]));
      },
      query ? 60 : 0,
    );
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [query, isOpen]);

  useEffect(() => setSelected(0), [results]);

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const close = () => {
    setQuickOpen(false);
    setQuery("");
    setResults([]);
  };

  const pick = (path: string) => {
    close();
    useApp.getState().setView("workspace");
    void useEditor.getState().openFile(path);
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[15vh]" onClick={close}>
      <div
        className="ticks w-full max-w-xl animate-rise overflow-hidden rounded-md border border-neutral-300 bg-white shadow-2xl dark:border-neutral-700 dark:bg-neutral-900"
        onClick={(e) => e.stopPropagation()}
      >
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
              e.preventDefault();
              setSelected((i) => Math.min(results.length - 1, i + 1));
            } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p" && isMac)) {
              e.preventDefault();
              setSelected((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter" && results[selected]) {
              e.preventDefault();
              pick(results[selected]);
            } else if (e.key === "Escape") {
              e.preventDefault();
              close();
            }
          }}
          placeholder="Search files by name…"
          spellCheck={false}
          className="w-full border-b border-neutral-200 bg-transparent px-4 py-3 text-sm focus:outline-none dark:border-neutral-800"
        />
        <div ref={listRef} className="max-h-80 overflow-y-auto p-1">
          {results.map((path, i) => {
            const slash = path.lastIndexOf("/");
            const name = path.slice(slash + 1);
            const dir = slash >= 0 ? path.slice(0, slash) : "";
            return (
              <button
                key={path}
                data-index={i}
                onClick={() => pick(path)}
                onMouseMove={() => setSelected(i)}
                className={`flex w-full items-center gap-2 rounded-md px-3 py-1.5 text-left text-xs ${
                  i === selected ? "bg-accent-500/10 text-accent-500 dark:text-accent-300" : ""
                }`}
              >
                <FileText size={13} className="shrink-0 opacity-60" />
                <span className="shrink-0 truncate">{name}</span>
                {dir && <span className="min-w-0 truncate font-mono text-[10px] text-neutral-500">{dir}</span>}
              </button>
            );
          })}
          {results.length === 0 && (
            <div className="px-3 py-6 text-center text-xs text-neutral-500">
              {query.trim() ? "No matching files" : "No files"}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
