/** Browser preview panel. The page itself is a native child webview owned
 *  by the backend; this component renders the toolbar and a placeholder
 *  whose on-screen rectangle the webview is kept glued to. */
import { useEffect, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, Copy, Globe, Loader2, RotateCw, X } from "lucide-react";
import { api, events } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { IconButton } from "@/components/ui";

const QUICK = ["localhost:3000", "localhost:5173", "localhost:8000", "localhost:8080"];

/** `id` must be unique per mounted panel (one per layout). */
export function BrowserPanel({ id }: { id: string }) {
  const workspaceId = useApp((s) => s.workspace?.id ?? "none");
  const storageKey = `jcode:browser-url:${workspaceId}`;
  const hostRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [address, setAddress] = useState(() => localStorage.getItem(storageKey) ?? "");
  const [current, setCurrent] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const openRef = useRef(false);

  const boundsOf = () => {
    const el = hostRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width < 4 || r.height < 4) return null;
    // The main webview is zoomed natively, so CSS px != window logical px.
    const z = useApp.getState().zoom;
    return { x: r.left * z, y: r.top * z, width: r.width * z, height: r.height * z };
  };

  const open = async (raw: string) => {
    const target = raw.trim();
    if (!target) return;
    setError(null);
    const bounds = boundsOf();
    if (!bounds) return;
    try {
      const url = openRef.current ? await api.browserNavigate(id, target) : await api.browserOpen(id, target, bounds);
      openRef.current = true;
      setCurrent(url);
      setAddress(url);
      localStorage.setItem(storageKey, url);
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  // Track URL / loading state reported by the native webview.
  useEffect(() => {
    const un = events.browserState((e) => {
      if (e.id !== id) return;
      setLoading(e.loading);
      setCurrent(e.url);
      if (document.activeElement !== inputRef.current) setAddress(e.url);
      if (!e.loading && e.url.startsWith("http")) localStorage.setItem(storageKey, e.url);
    });
    return () => void un.then((f) => f());
  }, [id, storageKey]);

  // Keep the native webview glued to the placeholder. Polling per frame is
  // cheap (one rect read) and catches every cause of movement: panel
  // resizes, sidebar drags, tab switches, zoom, window resizes.
  useEffect(() => {
    let frame = 0;
    let last = "";
    const tick = () => {
      frame = requestAnimationFrame(tick);
      if (!openRef.current) return;
      const b = boundsOf();
      const visible = b !== null && !useApp.getState().paletteOpen && document.visibilityState === "visible";
      const key = visible && b ? `${Math.round(b.x)},${Math.round(b.y)},${Math.round(b.width)},${Math.round(b.height)}` : "hidden";
      if (key === last) return;
      const wasHidden = last === "hidden" || last === "";
      last = key;
      if (!visible || !b) {
        void api.browserSetVisible(id, false).catch(() => {});
        return;
      }
      void api.browserSetBounds(id, b).catch(() => {});
      if (wasHidden) void api.browserSetVisible(id, true).catch(() => {});
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [id]);

  // Reopen the last previewed URL for this project once the panel is laid out.
  useEffect(() => {
    const saved = localStorage.getItem(storageKey);
    if (!saved) return;
    let tries = 0;
    const t = setInterval(() => {
      if (openRef.current || ++tries > 40) return clearInterval(t);
      if (boundsOf()) {
        clearInterval(t);
        void open(saved);
      }
    }, 50);
    return () => clearInterval(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);

  // Close the native webview with the panel.
  useEffect(() => {
    return () => {
      openRef.current = false;
      void api.browserClose(id).catch(() => {});
    };
  }, [id]);

  const action = (a: "back" | "forward" | "reload" | "stop") => void api.browserAction(id, a).catch(() => {});

  return (
    <div className="flex h-full min-h-0 flex-col bg-white dark:bg-[#0d0f14]">
      <form
        className="flex h-9 shrink-0 items-center gap-0.5 border-b border-neutral-200 px-1.5 dark:border-neutral-800"
        onSubmit={(e) => {
          e.preventDefault();
          void open(address);
          inputRef.current?.blur();
        }}
      >
        <IconButton type="button" title="Back" disabled={!current} onClick={() => action("back")}>
          <ArrowLeft size={13} />
        </IconButton>
        <IconButton type="button" title="Forward" disabled={!current} onClick={() => action("forward")}>
          <ArrowRight size={13} />
        </IconButton>
        <IconButton
          type="button"
          title={loading ? "Stop" : "Reload"}
          disabled={!current}
          onClick={() => action(loading ? "stop" : "reload")}
        >
          {loading ? <X size={13} /> : <RotateCw size={12} />}
        </IconButton>
        <div className="relative mx-1 min-w-0 flex-1">
          {loading ? (
            <Loader2 size={11} className="absolute left-2 top-1/2 -translate-y-1/2 animate-spin text-indigo-400" />
          ) : (
            <Globe size={11} className="absolute left-2 top-1/2 -translate-y-1/2 text-neutral-400" />
          )}
          <input
            ref={inputRef}
            value={address}
            onChange={(e) => setAddress(e.target.value)}
            onFocus={(e) => e.target.select()}
            placeholder="localhost:3000 or https://…"
            spellCheck={false}
            className="w-full rounded-md border border-neutral-200 bg-neutral-50 py-1 pl-6 pr-2 font-mono text-[11px] focus:border-indigo-500/60 focus:outline-none focus:ring-2 focus:ring-indigo-500/20 dark:border-neutral-800 dark:bg-[#101218]"
          />
        </div>
        <IconButton
          type="button"
          title="Copy URL"
          disabled={!current}
          onClick={() => current && void navigator.clipboard.writeText(current)}
        >
          <Copy size={12} />
        </IconButton>
      </form>
      {error && <div className="shrink-0 bg-red-500/10 px-3 py-1 text-[11px] text-red-400">{error}</div>}
      <div ref={hostRef} className="relative min-h-0 flex-1">
        {!current && (
          <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
            <Globe size={28} className="text-neutral-400" />
            <div className="text-sm font-medium">Preview your app</div>
            <div className="max-w-xs text-xs text-neutral-500">
              Enter a URL above — for example the dev server the agent just started.
            </div>
            <div className="flex flex-wrap justify-center gap-1.5">
              {QUICK.map((q) => (
                <button
                  key={q}
                  onClick={() => void open(q)}
                  className="rounded-md border border-neutral-200 px-2 py-1 font-mono text-[11px] text-neutral-600 hover:border-indigo-400/60 hover:text-indigo-500 dark:border-neutral-800 dark:text-neutral-300"
                >
                  {q}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
