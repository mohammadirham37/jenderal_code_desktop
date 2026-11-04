/** Remote control status (phone via relay), kept in sync with
 *  `remote://status`. Sessions created from the phone reload the list. */
import { create } from "zustand";
import { api, events, type RemoteStatus } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { useChat } from "@/stores/chat";

interface RemoteStore {
  status: RemoteStatus | null;
  refresh: () => Promise<void>;
  setStatus: (s: RemoteStatus) => void;
}

export const useRemote = create<RemoteStore>((set) => ({
  status: null,
  refresh: async () => {
    try {
      set({ status: await api.remoteStatus() });
    } catch {
      /* not running inside Tauri */
    }
  },
  setStatus: (status) => set({ status }),
}));

let started = false;
/** Subscribe once to the remote events (called from App). */
export function startRemoteSync() {
  if (started) return;
  started = true;
  void useRemote.getState().refresh();
  void events.remoteStatus((s) => useRemote.getState().setStatus(s)).catch(() => {});
  void events
    .remoteSessions(({ workspace_id }) => {
      if (useApp.getState().workspace?.id === workspace_id) void useChat.getState().loadSessions(workspace_id);
    })
    .catch(() => {});
}
