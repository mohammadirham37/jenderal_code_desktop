/** Interactive terminal panel backed by a backend PTY. */
import { useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { Plus, X } from "lucide-react";
import { api, events } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { IconButton } from "@/components/ui";

interface TermTab {
  id: string;
  title: string;
}

export function TerminalPanel() {
  const fontSize = useApp((s) => s.settings.terminal_font_size);
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const hostsRef = useRef<Map<string, { term: XTerm; fit: FitAddon }>>(new Map());
  const activeIdRef = useRef<string | null>(null);
  activeIdRef.current = activeId;
  const tabsRef = useRef<TermTab[]>([]);
  tabsRef.current = tabs;
  const creatingRef = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const newTerminal = async () => {
    if (creatingRef.current) return;
    creatingRef.current = true;
    try {
      const id = `term-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
      await api.terminalCreate(id, 80, 24);
      setTabs((t) => [...t, { id, title: `shell ${t.length + 1}` }]);
      setActiveId(id);
    } finally {
      creatingRef.current = false;
    }
  };

  const closeTerminal = async (id: string) => {
    await api.terminalKill(id);
    hostsRef.current.get(id)?.term.dispose();
    hostsRef.current.delete(id);
    setTabs((t) => {
      const next = t.filter((x) => x.id !== id);
      setActiveId(next.length ? next[next.length - 1].id : null);
      return next;
    });
  };

  // Forward PTY output into the right xterm instance.
  useEffect(() => {
    const un1 = events.terminalData((e) => {
      hostsRef.current.get(e.terminal_id)?.term.write(e.data);
    });
    const un2 = events.terminalExit((e) => {
      hostsRef.current
        .get(e.terminal_id)
        ?.term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n");
    });
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
    };
  }, []);

  // Auto-create the first terminal tab.
  useEffect(() => {
    if (tabs.length === 0) void newTerminal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabs.length === 0]);

  // Create/attach an xterm instance for the active tab.
  useEffect(() => {
    if (!activeId) return;
    const hostId = `host-${activeId}`;
    const timer = setInterval(() => {
      const el = document.getElementById(hostId);
      if (!el || hostsRef.current.has(activeId)) return;
      clearInterval(timer);
      const term = new XTerm({
        fontSize,
        fontFamily: "var(--font-mono)",
        cursorBlink: true,
        theme: {
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
        },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(el);
      requestAnimationFrame(() => fit.fit());
      term.onData((data) => void api.terminalWrite(activeId, data));
      term.onResize(({ cols, rows }) => {
        void api.terminalResize(activeId, cols, rows).catch(() => {});
      });
      hostsRef.current.set(activeId, { term, fit });
      term.focus();
    }, 30);
    return () => clearInterval(timer);
  }, [activeId, fontSize]);

  // A terminal that was hidden (display:none, zero size) must be refitted
  // when it becomes visible again — whether its own tab was switched or a
  // parent panel was shown/resized — so observe the container size.
  useEffect(() => {
    const refit = () => {
      const id = activeIdRef.current;
      const host = id ? hostsRef.current.get(id) : undefined;
      const el = containerRef.current;
      if (host && el && el.clientWidth > 0 && el.clientHeight > 0) host.fit.fit();
    };
    const t = setTimeout(refit, 60);
    const ro = new ResizeObserver(refit);
    if (containerRef.current) ro.observe(containerRef.current);
    return () => {
      clearTimeout(t);
      ro.disconnect();
    };
  }, [activeId]);

  // Apply font-size changes to live terminals.
  useEffect(() => {
    for (const { term, fit } of hostsRef.current.values()) {
      term.options.fontSize = fontSize;
      fit.fit();
    }
  }, [fontSize]);

  // Dispose all terminals and kill their shells on unmount, so PTYs don't
  // leak when the panel goes away.
  useEffect(() => {
    return () => {
      for (const t of tabsRef.current) void api.terminalKill(t.id).catch(() => {});
      for (const { term } of hostsRef.current.values()) term.dispose();
      hostsRef.current.clear();
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center border-b border-neutral-200 dark:border-neutral-800">
        {tabs.map((t) => (
          <div
            key={t.id}
            onClick={() => setActiveId(t.id)}
            className={`flex cursor-pointer items-center gap-1 px-3 text-[11px] ${
              activeId === t.id
                ? "border-b-2 border-b-accent-500 font-medium"
                : "text-neutral-500 hover:text-neutral-300"
            }`}
          >
            {t.title}
            <button
              className="rounded p-0.5 hover:bg-neutral-300/60 dark:hover:bg-neutral-700"
              onClick={(e) => {
                e.stopPropagation();
                void closeTerminal(t.id);
              }}
            >
              <X size={10} />
            </button>
          </div>
        ))}
        <IconButton title="New terminal" className="ml-1 h-6 w-6" onClick={() => void newTerminal()}>
          <Plus size={12} />
        </IconButton>
      </div>
      <div ref={containerRef} className="relative min-h-0 flex-1">
        {tabs.map((t) => (
          <div
            key={t.id}
            id={`host-${t.id}`}
            className="terminal-host absolute inset-0"
            style={{ display: activeId === t.id ? "block" : "none" }}
          />
        ))}
      </div>
    </div>
  );
}
