/** Terminal and agent-CLI tabs in the Chat layout's centre column. Tabs
 *  belong to their project: switching projects parks them (their processes
 *  keep running and their output keeps arriving in the xterm registry) and
 *  switching back restores them. They end when closed, when their project
 *  is closed or removed, or when the app quits. */
import { create } from "zustand";
import { api, events, type AgentCli } from "@/lib/ipc";
import { onWorkspaceRemoved, useApp } from "./app";

export const CHAT_TAB = "chat";

export interface CenterTab {
  id: string;
  title: string;
  /** Agent CLI command; absent for a plain shell. */
  command?: string;
  agentId?: string;
  /** Opened with the agent's bypass-permission flags. */
  bypass?: boolean;
  exited: boolean;
  error?: string;
}

interface ParkedTabs {
  tabs: CenterTab[];
  activeId: string;
}

interface CenterTabsState {
  /** Tabs of the open project. */
  tabs: CenterTab[];
  activeId: string;
  /** Tabs of the other projects, by workspace id. */
  parked: Record<string, ParkedTabs>;
  /** Installed state by agent id; `null` until detected. */
  installed: Record<string, boolean> | null;
  setActive: (id: string) => void;
  openTerminal: () => Promise<void>;
  openAgent: (agent: AgentCli, bypass?: boolean) => Promise<void>;
  restart: (id: string) => Promise<void>;
  close: (id: string) => Promise<void>;
  detectInstalled: () => Promise<void>;
  /** Park the open project's tabs and show `workspaceId`'s. */
  switchProject: (from: string | null, to: string | null) => void;
  /** Kill a project's tabs (open or parked). */
  killProject: (workspaceId: string) => void;
}

// xterm.js stays out of the initial bundle until a tab is opened.
const registry = () => import("@/features/terminal/xtermRegistry");

const newTabId = () => `center-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;

async function spawn(tab: CenterTab) {
  (await registry()).ensureXterm(tab.id, useApp.getState().settings.terminal_font_size);
  await api.terminalCreate(tab.id, 80, 24, tab.command);
}

let exitListening = false;

export const useCenterTabs = create<CenterTabsState>((set, get) => {
  // Exit events also reach tabs of parked projects.
  const patch = (id: string, p: Partial<CenterTab>) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id ? { ...t, ...p } : t)),
      parked: Object.fromEntries(
        Object.entries(s.parked).map(([ws, v]) => [
          ws,
          { ...v, tabs: v.tabs.map((t) => (t.id === id ? { ...t, ...p } : t)) },
        ]),
      ),
    }));

  const kill = (ids: string[]) => {
    for (const id of ids) void api.terminalKill(id).catch(() => {});
    if (ids.length) void registry().then((r) => ids.forEach(r.disposeXterm));
  };

  const open = async (tab: CenterTab) => {
    if (!exitListening) {
      exitListening = true;
      void events.terminalExit((e) => patch(e.terminal_id, { exited: true }));
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeId: tab.id }));
    try {
      await spawn(tab);
    } catch (e) {
      patch(tab.id, { exited: true, error: String(e) });
    }
  };

  return {
    tabs: [],
    activeId: CHAT_TAB,
    parked: {},
    installed: null,

    setActive: (activeId) => set({ activeId }),

    openTerminal: () => {
      const n = get().tabs.filter((t) => !t.command).length + 1;
      return open({ id: newTabId(), title: `shell ${n}`, exited: false });
    },

    openAgent: (agent, bypass = false) => {
      const flags = bypass ? agent.bypass_args?.trim() : "";
      return open({
        id: newTabId(),
        title: agent.name,
        command: flags ? `${agent.command} ${flags}` : agent.command,
        agentId: agent.id,
        bypass: !!flags,
        exited: false,
      });
    },

    restart: async (id) => {
      const tab = get().tabs.find((t) => t.id === id);
      if (!tab) return;
      patch(id, { exited: false, error: undefined });
      (await registry()).writeXterm(id, "\r\n\x1b[90m[restarted]\x1b[0m\r\n");
      try {
        await spawn(tab);
      } catch (e) {
        patch(id, { exited: true, error: String(e) });
      }
    },

    close: async (id) => {
      await api.terminalKill(id).catch(() => {});
      (await registry()).disposeXterm(id);
      set((s) => {
        const idx = s.tabs.findIndex((t) => t.id === id);
        const tabs = s.tabs.filter((t) => t.id !== id);
        const activeId = s.activeId !== id ? s.activeId : idx > 0 ? tabs[idx - 1].id : CHAT_TAB;
        return { tabs, activeId };
      });
    },

    detectInstalled: async () => {
      const agents = useApp.getState().settings.agent_clis;
      let found: boolean[] = [];
      try {
        found = await api.detectAgentClis(agents.map((a) => a.command));
      } catch {
        // Unknown: every agent shows as not installed but stays clickable.
      }
      set({ installed: Object.fromEntries(agents.map((a, i) => [a.id, found[i] ?? false])) });
    },

    switchProject: (from, to) => {
      set((s) => {
        const parked = { ...s.parked };
        if (from && s.tabs.length) parked[from] = { tabs: s.tabs, activeId: s.activeId };
        const next = (to && parked[to]) || { tabs: [], activeId: CHAT_TAB };
        if (to) delete parked[to];
        return { parked, tabs: next.tabs, activeId: next.activeId };
      });
    },

    killProject: (workspaceId) => {
      const ws = useApp.getState().workspace;
      const s = get();
      if (ws?.id === workspaceId) {
        kill(s.tabs.map((t) => t.id));
        set({ tabs: [], activeId: CHAT_TAB });
      }
      const parked = s.parked[workspaceId];
      if (parked) {
        kill(parked.tabs.map((t) => t.id));
        const rest = { ...s.parked };
        delete rest[workspaceId];
        set({ parked: rest });
      }
    },
  };
});

// Tabs follow their project: switching parks them, closing the project
// (workspace → none) kills them. The detection cache follows the agent list.
useApp.subscribe((s, prev) => {
  const from = prev.workspace?.id ?? null;
  const to = s.workspace?.id ?? null;
  if (from !== to) {
    if (from && !to) useCenterTabs.getState().killProject(from);
    else useCenterTabs.getState().switchProject(from, to);
  } else if (s.settings.agent_clis !== prev.settings.agent_clis) {
    useCenterTabs.setState({ installed: null });
  }
});

onWorkspaceRemoved((id) => useCenterTabs.getState().killProject(id));
