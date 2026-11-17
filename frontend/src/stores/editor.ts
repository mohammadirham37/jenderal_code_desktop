/** Editor tabs: files and diff views. Each project has its own: switching
 *  projects parks the open tabs (unsaved edits included), the expanded
 *  folders and language overrides, and brings back the other project's. */
import { create } from "zustand";
import { api } from "@/lib/ipc";
import { onWorkspaceRemoved, useApp } from "./app";

export interface FileTab {
  kind: "file";
  path: string;
  content: string;
  original: string;
  dirty: boolean;
}

export interface DiffTab {
  kind: "diff";
  /** Stable tab id. */
  key: string;
  title: string;
  /** Unified diff text. */
  diff: string;
}

export type Tab = FileTab | DiffTab;

interface EditorState {
  tabs: Tab[];
  activeKey: string | null;
  expandedDirs: Set<string>;
  /** Manual per-file language overrides for syntax highlighting. */
  langOverrides: Record<string, string>;
  /** Bumped by every openFile/openDiff, so layouts can reveal the editor
   *  even when the opened tab was already active. */
  revealTick: number;

  openFile: (path: string) => Promise<void>;
  openDiff: (key: string, title: string, diff: string) => Promise<void>;
  closeTab: (key: string) => void;
  setActive: (key: string) => void;
  update: (path: string, content: string) => void;
  save: (path: string) => Promise<void>;
  saveActive: () => Promise<void>;
  toggleDir: (path: string) => void;
  refreshFile: (path: string) => Promise<void>;
  setLanguage: (path: string, lang: string) => void;
  activeTab: () => Tab | null;
}

const fileKey = (path: string) => `file:${path}`;

export const useEditor = create<EditorState>((set, get) => ({
  tabs: [],
  activeKey: null,
  expandedDirs: new Set<string>(),
  langOverrides: {},
  revealTick: 0,

  openFile: async (path) => {
    const key = fileKey(path);
    const existing = get().tabs.find((t) => t.kind === "file" && fileKey(t.path) === key);
    if (existing) {
      set((s) => ({ activeKey: key, revealTick: s.revealTick + 1 }));
      return;
    }
    const ws = currentWorkspace();
    const res = await api.fsRead(path);
    // The project changed while reading: the file belongs to the old one.
    if (currentWorkspace() !== ws) return;
    if (get().tabs.some((t) => t.kind === "file" && t.path === path)) {
      set((s) => ({ activeKey: key, revealTick: s.revealTick + 1 }));
      return;
    }
    set((s) => ({
      revealTick: s.revealTick + 1,
      tabs: [
        ...s.tabs,
        { kind: "file", path, content: res.content, original: res.content, dirty: false },
      ],
      activeKey: key,
    }));
  },

  openDiff: async (key, title, diff) => {
    const existing = get().tabs.find((t) => t.kind === "diff" && t.key === key);
    if (existing) {
      set((s) => ({
        tabs: s.tabs.map((t) => (t.kind === "diff" && t.key === key ? { ...t, diff } : t)),
        activeKey: key,
        revealTick: s.revealTick + 1,
      }));
      return;
    }
    set((s) => ({ tabs: [...s.tabs, { kind: "diff", key, title, diff }], activeKey: key, revealTick: s.revealTick + 1 }));
  },

  closeTab: (key) => {
    set((s) => {
      const tabs = s.tabs.filter((t) => (t.kind === "file" ? fileKey(t.path) : t.key) !== key);
      const activeKey = s.activeKey === key ? null : s.activeKey;
      return { tabs, activeKey: activeKey ?? (tabs.length ? keyOf(tabs[tabs.length - 1]) : null) };
    });
  },

  setActive: (key) => set({ activeKey: key }),

  update: (path, content) => {
    set((s) => ({
      tabs: s.tabs.map((t) =>
        t.kind === "file" && t.path === path ? { ...t, content, dirty: content !== t.original } : t,
      ),
    }));
  },

  save: async (path) => {
    const tab = get().tabs.find((t) => t.kind === "file" && t.path === path);
    if (!tab || tab.kind !== "file") return;
    const ws = currentWorkspace();
    await api.fsWrite(path, tab.content);
    if (currentWorkspace() !== ws) {
      const p = parked[ws ?? ""];
      if (p) p.tabs = p.tabs.map((t) => (t.kind === "file" && t.path === path ? { ...t, original: tab.content, dirty: t.content !== tab.content } : t));
      return;
    }
    set((s) => ({
      tabs: s.tabs.map((t) =>
        t.kind === "file" && t.path === path ? { ...t, original: t.content, dirty: false } : t,
      ),
    }));
  },

  saveActive: async () => {
    const key = get().activeKey;
    if (!key) return;
    const tab = get().tabs.find((t) => keyOf(t) === key);
    if (tab && tab.kind === "file") await get().save(tab.path);
  },

  toggleDir: (path) => {
    set((s) => {
      const next = new Set(s.expandedDirs);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return { expandedDirs: next };
    });
  },

  refreshFile: async (path) => {
    const tab = get().tabs.find((t) => t.kind === "file" && t.path === path);
    if (!tab || tab.kind !== "file") return;
    if (tab.dirty) return; // do not clobber unsaved user edits
    try {
      const ws = currentWorkspace();
      const res = await api.fsRead(path);
      if (currentWorkspace() !== ws) return;
      set((s) => ({
        tabs: s.tabs.map((t) =>
          t.kind === "file" && t.path === path
            ? { ...t, content: res.content, original: res.content, dirty: false }
            : t,
        ),
      }));
    } catch {
      // File may have been deleted; leave the tab as-is.
    }
  },

  setLanguage: (path, lang) => {
    set((s) => ({ langOverrides: { ...s.langOverrides, [path]: lang } }));
  },

  activeTab: () => {
    const key = get().activeKey;
    return get().tabs.find((t) => keyOf(t) === key) ?? null;
  },
}));

function keyOf(t: Tab): string {
  return t.kind === "file" ? fileKey(t.path) : t.key;
}

function currentWorkspace(): string | null {
  return useApp.getState().workspace?.id ?? null;
}

interface ParkedEditor {
  tabs: Tab[];
  activeKey: string | null;
  expandedDirs: Set<string>;
  langOverrides: Record<string, string>;
}

/** Editor state of the projects that are not shown, by workspace id. */
const parked: Record<string, ParkedEditor> = {};

useApp.subscribe((s, prev) => {
  const from = prev.workspace?.id ?? null;
  const to = s.workspace?.id ?? null;
  if (from === to) return;
  const cur = useEditor.getState();
  if (from) {
    parked[from] = {
      tabs: cur.tabs,
      activeKey: cur.activeKey,
      expandedDirs: cur.expandedDirs,
      langOverrides: cur.langOverrides,
    };
  }
  const next = to ? parked[to] : undefined;
  if (to) delete parked[to];
  useEditor.setState({
    tabs: next?.tabs ?? [],
    activeKey: next?.activeKey ?? null,
    expandedDirs: next?.expandedDirs ?? new Set<string>(),
    langOverrides: next?.langOverrides ?? {},
  });
});

onWorkspaceRemoved((id) => {
  delete parked[id];
});
