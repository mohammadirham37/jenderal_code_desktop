/** Chat/session state: messages, streaming, tool runs, approvals. */
import { create } from "zustand";
import {
  api,
  events,
  type AgentProfile,
  type ApprovalRecord,
  type ChatStatus,
  type MessageRecord,
  type SessionSummary,
  type ToolRunRecord,
} from "@/lib/ipc";

interface ChatState {
  sessions: SessionSummary[];
  activeSessionId: string | null;
  messages: MessageRecord[];
  toolRuns: ToolRunRecord[];
  approvals: ApprovalRecord[];
  profiles: AgentProfile[];
  /** Streaming assistant text being assembled from deltas. */
  streaming: { messageId: string; text: string } | null;
  status: ChatStatus;
  statusDetail: string | null;
  lastError: string | null;
  usage: { input: number; output: number; cost: number | null };
  initialized: boolean;

  init: () => Promise<void>;
  loadSessions: (workspaceId?: string) => Promise<void>;
  loadProfiles: () => Promise<void>;
  selectSession: (id: string) => Promise<void>;
  newSession: (workspaceId: string, agentProfileId?: string, providerId?: string, modelName?: string) => Promise<string>;
  send: (text: string, attachments: string[]) => Promise<void>;
  cancel: () => Promise<void>;
  decide: (approvalId: string, approved: boolean, grant?: string) => Promise<void>;
  setModel: (providerId: string, modelName: string) => Promise<void>;
  setAgent: (profileId: string) => Promise<void>;
  setPolicy: (mode: string) => Promise<void>;
  rename: (id: string, title: string) => Promise<void>;
  removeSession: (id: string) => Promise<void>;
  saveAgentProfile: (profile: AgentProfile) => Promise<void>;
  deleteAgentProfile: (id: string) => Promise<void>;
  activeSession: () => SessionSummary | null;
}

/** Coalesce session-list refreshes triggered by bursts of message events. */
let refreshTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleSessionsRefresh(load: () => Promise<void>) {
  if (refreshTimer) clearTimeout(refreshTimer);
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    void load().catch(() => {});
  }, 400);
}

export const useChat = create<ChatState>((set, get) => ({
  sessions: [],
  activeSessionId: null,
  messages: [],
  toolRuns: [],
  approvals: [],
  profiles: [],
  streaming: null,
  status: "idle",
  statusDetail: null,
  lastError: null,
  usage: { input: 0, output: 0, cost: null },
  initialized: false,

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await Promise.all([get().loadSessions(), get().loadProfiles()]);

    await events.chatStatus((e) => {
      if (e.session_id !== get().activeSessionId) return;
      set({ status: e.status, statusDetail: e.detail ?? null });
    });
    // Deltas arrive per token; coalesce them into one store update per
    // animation frame so the chat re-renders at display rate, not token rate.
    let pending: { messageId: string; text: string } | null = null;
    let frame: number | null = null;
    const flush = () => {
      frame = null;
      if (!pending) return;
      const { messageId, text } = pending;
      pending = null;
      const cur = get().streaming;
      set({
        streaming:
          cur && cur.messageId === messageId
            ? { messageId, text: cur.text + text }
            : { messageId, text },
      });
    };
    await events.chatDelta((e) => {
      if (e.session_id !== get().activeSessionId) return;
      if (pending && pending.messageId !== e.message_id) flush();
      pending = pending
        ? { messageId: e.message_id, text: pending.text + e.delta }
        : { messageId: e.message_id, text: e.delta };
      if (frame === null) frame = requestAnimationFrame(flush);
    });
    await events.chatMessage((e) => {
      if (e.session_id !== get().activeSessionId) {
        scheduleSessionsRefresh(get().loadSessions);
        return;
      }
      if (pending && pending.messageId === e.message.id) {
        pending = null;
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null;
      }
      set((s) => {
        if (s.messages.some((m) => m.id === e.message.id)) return {};
        const streaming = s.streaming;
        const messages = [...s.messages, e.message];
        const cleared =
          streaming && streaming.messageId === e.message.id ? null : streaming;
        return { messages, streaming: cleared };
      });
      scheduleSessionsRefresh(get().loadSessions);
    });
    await events.chatTool((e) => {
      if (e.session_id !== get().activeSessionId) return;
      set((s) => {
        const idx = s.toolRuns.findIndex((r) => r.id === e.tool_run.id);
        const toolRuns = [...s.toolRuns];
        if (idx >= 0) toolRuns[idx] = e.tool_run;
        else toolRuns.push(e.tool_run);
        return { toolRuns };
      });
    });
    await events.chatApproval((e) => {
      if (e.session_id !== get().activeSessionId) return;
      set((s) => {
        const idx = s.approvals.findIndex((a) => a.id === e.approval.id);
        const approvals = [...s.approvals];
        if (idx >= 0) approvals[idx] = e.approval;
        else approvals.push(e.approval);
        return { approvals };
      });
    });
    await events.chatError((e) => {
      if (e.session_id !== get().activeSessionId) return;
      pending = null;
      set({ lastError: e.error, status: "idle", streaming: null });
    });
  },

  loadSessions: async (workspaceId) => {
    set({ sessions: await api.listSessions(workspaceId) });
  },

  loadProfiles: async () => {
    set({ profiles: await api.listAgentProfiles() });
  },

  selectSession: async (id) => {
    const detail = await api.getSessionDetail(id);
    set({
      activeSessionId: id,
      messages: detail.messages,
      toolRuns: detail.tool_runs,
      approvals: detail.approvals,
      streaming: null,
      status: "idle",
      statusDetail: null,
      lastError: null,
    });
  },

  newSession: async (workspaceId, agentProfileId, providerId, modelName) => {
    const s = await api.createSession(workspaceId, agentProfileId, providerId, modelName);
    await get().selectSession(s.id);
    await get().loadSessions(workspaceId);
    return s.id;
  },

  send: async (text, attachments) => {
    const id = get().activeSessionId;
    if (!id) throw new Error("no active session");
    set({ lastError: null, status: "thinking" });
    await api.sendMessage(id, text, attachments);
  },

  cancel: async () => {
    const id = get().activeSessionId;
    if (!id) return;
    await api.cancelGeneration(id);
  },

  decide: async (approvalId, approved, grant) => {
    set((s) => ({
      approvals: s.approvals.map((a) =>
        a.id === approvalId ? { ...a, status: approved ? "approved" : "denied" } : a,
      ),
    }));
    await api.decideApproval(approvalId, approved, grant);
  },

  setModel: async (providerId, modelName) => {
    const id = get().activeSessionId;
    if (!id) return;
    await api.setSessionModel(id, providerId, modelName);
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, provider_id: providerId, model_name: modelName } : x,
      ),
    }));
  },

  setAgent: async (profileId) => {
    const id = get().activeSessionId;
    if (!id) return;
    await api.setSessionAgent(id, profileId);
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, agent_profile_id: profileId } : x,
      ),
    }));
  },

  setPolicy: async (mode) => {
    const id = get().activeSessionId;
    if (!id) return;
    await api.setPolicyMode(id, mode);
    set((s) => ({
      sessions: s.sessions.map((x) =>
        x.id === id ? { ...x, policy_mode: mode as SessionSummary["policy_mode"] } : x,
      ),
    }));
  },

  rename: async (id, title) => {
    await api.renameSession(id, title);
    await get().loadSessions();
  },

  removeSession: async (id) => {
    await api.deleteSession(id);
    if (get().activeSessionId === id) {
      set({
        activeSessionId: null,
        messages: [],
        toolRuns: [],
        approvals: [],
        streaming: null,
      });
    }
    await get().loadSessions();
  },

  activeSession: () => {
    const id = get().activeSessionId;
    return get().sessions.find((s) => s.id === id) ?? null;
  },

  saveAgentProfile: async (profile) => {
    await api.saveAgentProfile(profile);
    await get().loadProfiles();
  },

  deleteAgentProfile: async (id) => {
    await api.deleteAgentProfile(id);
    await get().loadProfiles();
  },
}));
