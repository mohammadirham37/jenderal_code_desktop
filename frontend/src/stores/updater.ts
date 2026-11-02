/** In-app updates (tauri-plugin-updater): checks the GitHub release feed
 *  (`latest.json`, see docs/RELEASE.md) at startup and every few hours,
 *  then downloads, verifies (minisign) and installs on request and restarts. */
import { create } from "zustand";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type Status = "idle" | "checking" | "current" | "available" | "downloading" | "restarting" | "error";

interface UpdaterState {
  status: Status;
  /** The running version. */
  current: string | null;
  update: Update | null;
  downloaded: number;
  /** Download size, when the server reports it. */
  total: number | null;
  error: string | null;
  dialogOpen: boolean;
  /** Look for a new release. Quiet checks keep errors (offline, no
   *  release yet) out of the UI. */
  checkNow: (quiet?: boolean) => Promise<void>;
  install: () => Promise<void>;
  setDialogOpen: (open: boolean) => void;
}

/** The launch check runs right away (after the IPC bridge is up). */
const FIRST_CHECK_MS = 1_500;
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

export const useUpdater = create<UpdaterState>((set, get) => ({
  status: "idle",
  current: null,
  update: null,
  downloaded: 0,
  total: null,
  error: null,
  dialogOpen: false,

  checkNow: async (quiet = false) => {
    const { status } = get();
    if (status === "checking" || status === "downloading" || status === "restarting") return;
    set({ status: "checking", error: null });
    try {
      const update = await check({ timeout: 30_000 });
      set({ update, status: update ? "available" : "current" });
    } catch (e) {
      set({ status: quiet ? "idle" : "error", error: quiet ? null : String(e) });
    }
  },

  install: async () => {
    const update = get().update;
    if (!update || get().status === "downloading") return;
    set({ status: "downloading", downloaded: 0, total: null, error: null });
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") set({ total: e.data.contentLength ?? null });
        else if (e.event === "Progress") set((s) => ({ downloaded: s.downloaded + e.data.chunkLength }));
      });
      set({ status: "restarting" });
      await relaunch();
    } catch (e) {
      set({ status: "error", error: String(e) });
    }
  },

  setDialogOpen: (dialogOpen) => set({ dialogOpen }),
}));

let started = false;

/** Start the periodic checks (once, at app start). */
export function startUpdateChecks() {
  if (started) return;
  started = true;
  void getVersion().then((current) => useUpdater.setState({ current }));
  // Not in `vite dev` without Tauri, and not while developing the app.
  if (import.meta.env.DEV) return;
  const quietCheck = async () => {
    if (useUpdater.getState().status !== "available") await useUpdater.getState().checkNow(true);
  };
  // At launch, a new version opens the dialog right away; later checks
  // only light up the sidebar button, so they never interrupt work.
  setTimeout(() => {
    void quietCheck().then(() => {
      if (useUpdater.getState().status === "available") useUpdater.getState().setDialogOpen(true);
    });
  }, FIRST_CHECK_MS);
  setInterval(() => void quietCheck(), CHECK_EVERY_MS);
}
