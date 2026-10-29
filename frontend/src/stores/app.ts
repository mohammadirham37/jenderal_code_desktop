/** Global app state: routing, workspace, settings, theme. */
import { create } from "zustand";
import { api, type AppSettings, type WorkspaceInfo } from "@/lib/ipc";
import { open } from "@tauri-apps/plugin-dialog";

export type View = "welcome" | "workspace" | "providers" | "agents" | "sessions" | "settings";

/** Workspace layout: IDE-style "code" or chat-dominant "chat". */
export type LayoutMode = "code" | "chat";
const LAYOUT_STORAGE_KEY = "jcode:layout-mode";

function readStoredLayout(): LayoutMode {
  return localStorage.getItem(LAYOUT_STORAGE_KEY) === "code" ? "code" : "chat";
}

/** UI zoom (webview zoom factor), persisted per device. */
const ZOOM_STORAGE_KEY = "jcode:zoom-factor";
export const ZOOM_MIN = 0.7;
export const ZOOM_MAX = 1.6;
export const ZOOM_STEP = 0.1;

function readStoredZoom(): number {
  const raw = parseFloat(localStorage.getItem(ZOOM_STORAGE_KEY) ?? "1");
  if (!Number.isFinite(raw)) return 1;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(raw * 100) / 100));
}

interface AppState {
  view: View;
  workspace: WorkspaceInfo | null;
  recent: WorkspaceInfo[];
  settings: AppSettings;
  paletteOpen: boolean;
  ready: boolean;
  zoom: number;
  layout: LayoutMode;

  init: () => Promise<void>;
  setView: (v: View) => void;
  setLayout: (mode: LayoutMode) => void;
  setPaletteOpen: (open: boolean) => void;
  openWorkspacePath: (path: string) => Promise<void>;
  openFolderPicker: () => Promise<void>;
  closeWorkspace: () => Promise<void>;
  removeRecent: (id: string) => Promise<void>;
  updateSettings: (patch: Partial<AppSettings>) => Promise<void>;
  setZoom: (factor: number) => Promise<void>;
  zoomIn: () => Promise<void>;
  zoomOut: () => Promise<void>;
  resetZoom: () => Promise<void>;
}

function applyTheme(theme: string) {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export const useApp = create<AppState>((set, get) => ({
  view: "welcome",
  workspace: null,
  recent: [],
  settings: {
    theme: "dark",
    default_policy_mode: "safe",
    editor_font_size: 13,
    terminal_font_size: 12,
    max_iterations: 100,
    retention_days: null,
    show_hidden_files: false,
    credential_storage: "file",
    editor_word_wrap: true,
  },
  paletteOpen: false,
  ready: false,
  zoom: readStoredZoom(),
  layout: readStoredLayout(),

  init: async () => {
    // Re-apply the persisted zoom (the early attempt at import time may have
    // raced the IPC bridge).
    void get().setZoom(readStoredZoom());
    try {
      const [settings, workspace, recent] = await Promise.all([
        api.getSettings(),
        api.getActiveWorkspace(),
        api.listWorkspaces(),
      ]);
      applyTheme(settings.theme);
      set({
        settings,
        workspace,
        recent,
        view: workspace ? "workspace" : "welcome",
        ready: true,
      });
    } catch (e) {
      console.error("init failed", e);
      set({ ready: true });
    }
  },

  setView: (v) => set({ view: v }),
  setLayout: (layout) => {
    localStorage.setItem(LAYOUT_STORAGE_KEY, layout);
    set({ layout });
  },
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),

  openWorkspacePath: async (path) => {
    const ws = await api.openWorkspace(path);
    const recent = await api.listWorkspaces();
    set({ workspace: ws, recent, view: "workspace" });
  },

  openFolderPicker: async () => {
    const selected = await open({ directory: true, multiple: false, title: "Open project folder" });
    if (typeof selected === "string" && selected.length > 0) {
      await get().openWorkspacePath(selected);
    }
  },

  closeWorkspace: async () => {
    await api.closeWorkspace();
    set({ workspace: null, view: "welcome" });
  },

  removeRecent: async (id) => {
    await api.removeWorkspace(id);
    set({ recent: await api.listWorkspaces() });
  },

  updateSettings: async (patch) => {
    const next = { ...get().settings, ...patch };
    await api.saveSettings(next);
    applyTheme(next.theme);
    set({ settings: next });
  },

  setZoom: async (factor) => {
    const zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(factor * 100) / 100));
    localStorage.setItem(ZOOM_STORAGE_KEY, String(zoom));
    set({ zoom });
    try {
      await api.setZoom(zoom);
    } catch (e) {
      console.error("set zoom failed", e);
    }
  },

  zoomIn: async () => {
    await get().setZoom(get().zoom + ZOOM_STEP);
  },

  zoomOut: async () => {
    await get().setZoom(get().zoom - ZOOM_STEP);
  },

  resetZoom: async () => {
    await get().setZoom(1);
  },
}));

// Apply the persisted zoom as early as possible so the app boots at the
// user's chosen size instead of flashing at 100%.
void useApp.getState().setZoom(readStoredZoom());
