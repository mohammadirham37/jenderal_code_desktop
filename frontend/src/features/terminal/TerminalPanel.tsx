/** Interactive terminal panel backed by a backend PTY. */
import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { api } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { IconButton } from "@/components/ui";
import { XtermView } from "./XtermView";
import { disposeXterm, ensureXterm, newTerminalId } from "./xtermRegistry";

interface TermTab {
  id: string;
  title: string;
}

export function TerminalPanel() {
  const [tabs, setTabs] = useState<TermTab[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const tabsRef = useRef<TermTab[]>([]);
  tabsRef.current = tabs;
  const creatingRef = useRef(false);

  const newTerminal = async () => {
    if (creatingRef.current) return;
    creatingRef.current = true;
    const id = newTerminalId();
    try {
      ensureXterm(id, useApp.getState().settings.terminal_font_size);
      const title = `shell ${tabs.length + 1}`;
      await api.terminalCreate(id, 80, 24, undefined, title);
      setTabs((t) => [...t, { id, title }]);
      setActiveId(id);
    } catch (e) {
      disposeXterm(id);
      throw e;
    } finally {
      creatingRef.current = false;
    }
  };

  const closeTerminal = async (id: string) => {
    await api.terminalKill(id);
    disposeXterm(id);
    setTabs((t) => {
      const next = t.filter((x) => x.id !== id);
      setActiveId(next.length ? next[next.length - 1].id : null);
      return next;
    });
  };

  // Auto-create the first terminal tab.
  useEffect(() => {
    if (tabs.length === 0) void newTerminal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabs.length === 0]);

  // Kill this panel's shells on unmount, so PTYs don't leak when the panel
  // goes away.
  useEffect(() => {
    return () => {
      for (const t of tabsRef.current) {
        void api.terminalKill(t.id).catch(() => {});
        disposeXterm(t.id);
      }
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
                : "text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-300"
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
      <div className="relative min-h-0 flex-1">
        {tabs.map((t) => (
          <XtermView key={t.id} terminalId={t.id} active={activeId === t.id} />
        ))}
      </div>
    </div>
  );
}
