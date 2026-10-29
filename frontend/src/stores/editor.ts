/** Editor tabs: files and diff views in the center pane. */
import { create } from "zustand";
import { api } from "@/lib/ipc";

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

  openFile: async (path) => {
    const key = fileKey(path);
    const existing = get().tabs.find((t) => t.kind === "file" && fileKey(t.path) === key);
    if (existing) {
      set({ activeKey: key });
      return;
    }
    const res = await api.fsRead(path);
    set((s) => ({
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
      }));
      return;
    }
    set((s) => ({ tabs: [...s.tabs, { kind: "diff", key, title, diff }], activeKey: key }));
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
    await api.fsWrite(path, tab.content);
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
      const res = await api.fsRead(path);
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
