/** Shells of the side/bottom Terminal panel, per project. Switching projects
 *  only changes which project's shells the panel shows; the others keep
 *  running. A project's shells end when they are closed, when the project
 *  is closed or removed from the list, or when the app quits. Both layouts
 *  show the same shells. */
import { create } from "zustand";
import { api } from "@/lib/ipc";
import { onWorkspaceRemoved, useApp } from "@/stores/app";
import { disposeXterm, ensureXterm, newTerminalId } from "./xtermRegistry";

export interface PanelTerminal {
  id: string;
  title: string;
}

export interface ProjectTerminals {
  tabs: PanelTerminal[];
  activeId: string | null;
  /** Number for the next "shell N" title. */
  next: number;
}

interface PanelTerminalsState {
  byWorkspace: Record<string, ProjectTerminals>;
  create: (workspaceId: string) => Promise<void>;
  close: (workspaceId: string, id: string) => Promise<void>;
  setActive: (workspaceId: string, id: string) => void;
  killProject: (workspaceId: string) => void;
}

const EMPTY: ProjectTerminals = { tabs: [], activeId: null, next: 1 };
const creating = new Set<string>();

export const usePanelTerminals = create<PanelTerminalsState>((set, get) => {
  const update = (ws: string, f: (p: ProjectTerminals) => ProjectTerminals) =>
    set((s) => ({ byWorkspace: { ...s.byWorkspace, [ws]: f(s.byWorkspace[ws] ?? EMPTY) } }));

  return {
    byWorkspace: {},

    create: async (ws) => {
      // The PTY starts in the open project's folder.
      if (creating.has(ws) || useApp.getState().workspace?.id !== ws) return;
      creating.add(ws);
      const id = newTerminalId();
      const title = `shell ${(get().byWorkspace[ws] ?? EMPTY).next}`;
      try {
        ensureXterm(id, useApp.getState().settings.terminal_font_size);
        await api.terminalCreate(id, 80, 24, undefined, title);
        update(ws, (p) => ({ tabs: [...p.tabs, { id, title }], activeId: id, next: p.next + 1 }));
      } catch (e) {
        disposeXterm(id);
        throw e;
      } finally {
        creating.delete(ws);
      }
    },

    close: async (ws, id) => {
      await api.terminalKill(id).catch(() => {});
      disposeXterm(id);
      update(ws, (p) => {
        const tabs = p.tabs.filter((t) => t.id !== id);
        const activeId = p.activeId === id ? (tabs.length ? tabs[tabs.length - 1].id : null) : p.activeId;
        return { ...p, tabs, activeId };
      });
    },

    setActive: (ws, id) => update(ws, (p) => ({ ...p, activeId: id })),

    killProject: (ws) => {
      const p = get().byWorkspace[ws];
      if (!p) return;
      for (const t of p.tabs) {
        void api.terminalKill(t.id).catch(() => {});
        disposeXterm(t.id);
      }
      set((s) => {
        const rest = { ...s.byWorkspace };
        delete rest[ws];
        return { byWorkspace: rest };
      });
    },
  };
});

// Closing a project (workspace → none) or removing it ends its shells.
useApp.subscribe((s, prev) => {
  const from = prev.workspace?.id ?? null;
  if (from && !s.workspace) usePanelTerminals.getState().killProject(from);
});
onWorkspaceRemoved((id) => usePanelTerminals.getState().killProject(id));
