/** Interactive terminal panel backed by backend PTYs: the open project's
 *  shells (`panelTerminals.ts`; each project has its own). */
import { useEffect } from "react";
import { Plus, X } from "lucide-react";
import { useApp } from "@/stores/app";
import { IconButton } from "@/components/ui";
import { XtermView } from "./XtermView";
import { usePanelTerminals, type ProjectTerminals } from "./panelTerminals";

const NONE: ProjectTerminals = { tabs: [], activeId: null, next: 1 };

/** `visible`: the panel is on screen; the first shell of a project is
 *  started only then. */
export function TerminalPanel({ visible }: { visible: boolean }) {
  const ws = useApp((s) => s.workspace?.id ?? null);
  const project = usePanelTerminals((s) => (ws ? s.byWorkspace[ws] : undefined)) ?? NONE;
  const { create, close, setActive } = usePanelTerminals.getState();
  const { tabs, activeId } = project;

  // Start the project's first shell (again after the last one is closed).
  useEffect(() => {
    if (visible && ws && tabs.length === 0) void create(ws);
  }, [visible, ws, tabs.length, create]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center border-b border-neutral-200 dark:border-neutral-800">
        {tabs.map((t) => (
          <div
            key={t.id}
            onClick={() => ws && setActive(ws, t.id)}
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
                if (ws) void close(ws, t.id);
              }}
            >
              <X size={10} />
            </button>
          </div>
        ))}
        <IconButton title="New terminal" className="ml-1 h-6 w-6" onClick={() => ws && void create(ws)}>
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
