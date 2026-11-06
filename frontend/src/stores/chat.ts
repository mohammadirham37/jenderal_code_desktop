/** Chat/session state: messages, streaming, tool runs, approvals. */
import { create } from "zustand";
import {
  api,
  events,
  type AgentProfile,
  type ApprovalRecord,
  type ChatStatus,
  type ImageAttachment,
  type MessageRecord,
  type SessionSummary,
  type SessionUsage,
  type ToolRunRecord,
} from "@/lib/ipc";

export interface StreamingMessage {
  messageId: string;
  text: string;
  reasoning: string;
}

/** A message written while the agent was busy, sent when its turn ends. */
export interface QueuedMessage {
  id: string;
  text: string;
  attachments: string[];
  images: ImageAttachment[];
}

/** A steering message the runtime accepted; it joins the running turn at
 *  the next step boundary (and disappears once its user message arrives). */
export interface SteeringMessage {
  id: string;
  text: string;
}

interface ChatState {
  sessions: SessionSummary[];
  activeSessionId: string | null;
  messages: MessageRecord[];
  toolRuns: ToolRunRecord[];
  approvals: ApprovalRecord[];
  profiles: AgentProfile[];
  /** Streaming assistant text (and reasoning) assembled from deltas. */
  streaming: StreamingMessage | null;
  status: ChatStatus;
  statusDetail: string | null;
  lastError: string | null;
  /** Token usage of the active session (null until loaded). */
  usage: SessionUsage | null;
  initialized: boolean;
  /** The session list has been loaded at least once. */
  sessionsLoaded: boolean;
  /** Per session: messages waiting for the running turn to end. */
  queued: Record<string, QueuedMessage[]>;
  /** Per session: steering messages not yet added to the conversation. */
  steering: Record<string, SteeringMessage[]>;
  /** Per session: the queue is held (after Stop or an error) until the
   *  user sends something. */
  queueHeld: Record<string, boolean>;

  init: () => Promise<void>;
  loadSessions: (workspaceId?: string) => Promise<void>;
  loadProfiles: () => Promise<void>;
  selectSession: (id: string) => Promise<void>;
  newSession: (workspaceId: string, agentProfileId?: string, providerId?: string, modelName?: string) => Promise<string>;
  send: (text: string, attachments: string[], images?: ImageAttachment[]) => Promise<void>;
  cancel: () => Promise<void>;
  /** Send to a specific session (the active one or a queued one). */
  sendTo: (sessionId: string, text: string, attachments: string[], images?: ImageAttachment[]) => Promise<void>;
  /** A session's turn ended: requeue unused steering, send the next. */
  turnEnded: (sessionId: string, hold?: boolean) => void;
  enqueue: (text: string, attachments: string[], images?: ImageAttachment[]) => void;
  removeQueued: (id: string) => void;
  /** Steer the running turn with `text` (falls back to the queue). */
  steer: (text: string) => Promise<void>;
  /** Move a queued message into the running turn. */
  steerQueued: (id: string) => Promise<void>;
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

/** The session last open in a project, selected again when it reopens. */
export const lastSessionKey = (workspaceId: string) => `jcode:last-session:${workspaceId}`;

/** Sessions with a running turn, from status events of every session. */
const running = new Set<string>();

/** `send_message` can race the runtime releasing the session right after
 *  the idle event, so retry briefly while it reports a running turn. */
async function sendWhenFree(sessionId: string, text: string, attachments: string[], images: ImageAttachment[]) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await api.sendMessage(sessionId, text, attachments, images);
    } catch (e) {
      if (attempt >= 20 || !String(e).includes("already running")) throw e;
      await new Promise((r) => setTimeout(r, 100));
    }
  }
}

/** Recent user message ids from events (see `steer`). */
const seenUserMessages = new Set<string>();

let nextLocalId = 0;
const localId = () => `local-${Date.now()}-${nextLocalId++}`;

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
  usage: null,
  initialized: false,
  sessionsLoaded: false,
  queued: {},
  steering: {},
  queueHeld: {},

  init: async () => {
    if (get().initialized) return;
    set({ initialized: true });
    await Promise.all([get().loadSessions(), get().loadProfiles()]);

    await events.chatStatus((e) => {
      if (e.status === "idle") {
        if (running.has(e.session_id)) get().turnEnded(e.session_id, e.detail === "cancelled");
      } else {
        running.add(e.session_id);
      }
      if (e.session_id !== get().activeSessionId) return;
      set({ status: e.status, statusDetail: e.detail ?? null });
    });
    // Deltas arrive per token; coalesce them into one store update per
    // animation frame so the chat re-renders at display rate, not token rate.
    let pending: StreamingMessage | null = null;
    let frame: number | null = null;
    const flush = () => {
      frame = null;
      if (!pending) return;
      const { messageId, text, reasoning } = pending;
      pending = null;
      const cur = get().streaming;
      set({
        streaming:
          cur && cur.messageId === messageId
            ? { messageId, text: cur.text + text, reasoning: cur.reasoning + reasoning }
            : { messageId, text, reasoning },
      });
    };
    await events.chatDelta((e) => {
      if (e.session_id !== get().activeSessionId) return;
      if (pending && pending.messageId !== e.message_id) flush();
      if (!pending) pending = { messageId: e.message_id, text: "", reasoning: "" };
      if (e.reasoning) pending.reasoning += e.delta;
      else pending.text += e.delta;
      if (frame === null) frame = requestAnimationFrame(flush);
    });
    await events.chatMessage((e) => {
      if (e.message.role === "user") {
        seenUserMessages.add(e.message.id);
        if (seenUserMessages.size > 200) seenUserMessages.delete(seenUserMessages.values().next().value!);
      }
      const steering = get().steering[e.session_id];
      if (steering?.some((m) => m.id === e.message.id)) {
        set((s) => ({
          steering: { ...s.steering, [e.session_id]: steering.filter((m) => m.id !== e.message.id) },
        }));
      }
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
        // Keep creation order: a compaction summary is created after, but
        // dated just before, the request that triggered it.
        const messages = [...s.messages, e.message].sort((a, b) => a.created_at - b.created_at);
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
    await events.chatUsage((e) => {
      if (e.session_id !== get().activeSessionId) return;
      set((s) => {
        const u = s.usage ?? { total_input: 0, total_output: 0, total_cost: null, requests: 0, last_input: 0, last_output: 0 };
        const cost = e.cost_estimate ?? null;
        return {
          usage: {
            total_input: u.total_input + e.usage.input_tokens,
            total_output: u.total_output + e.usage.output_tokens,
            total_cost: cost === null ? (u.total_cost ?? null) : (u.total_cost ?? 0) + cost,
            requests: u.requests + 1,
            last_input: e.usage.input_tokens,
            last_output: e.usage.output_tokens,
          },
        };
      });
    });
    await events.chatError((e) => {
      // The idle status that follows must not send the next queued message.
      set((s) => ({ queueHeld: { ...s.queueHeld, [e.session_id]: true } }));
      if (e.session_id !== get().activeSessionId) return;
      pending = null;
      set({ lastError: e.error, status: "idle", streaming: null });
    });
  },

  loadSessions: async (workspaceId) => {
    set({ sessions: await api.listSessions(workspaceId), sessionsLoaded: true });
  },

  loadProfiles: async () => {
    set({ profiles: await api.listAgentProfiles() });
  },

  selectSession: async (id) => {
    const [detail, usage] = await Promise.all([
      api.getSessionDetail(id),
      api.getSessionUsage(id).catch(() => null),
    ]);
    set({
      activeSessionId: id,
      usage,
      messages: detail.messages,
      toolRuns: detail.tool_runs,
      approvals: detail.approvals,
      streaming: null,
      // A turn may still be running in the session we switch to.
      status: running.has(id) ? "thinking" : "idle",
      statusDetail: null,
      lastError: null,
    });
    localStorage.setItem(lastSessionKey(detail.session.workspace_id), id);
  },

  newSession: async (workspaceId, agentProfileId, providerId, modelName) => {
    const s = await api.createSession(workspaceId, agentProfileId, providerId, modelName);
    // In the list before it becomes active: ChatPanel's "ensure a session"
    // effect switches away from an active session it cannot find, which
    // made the first click on New conversation look like it did nothing.
    set((st) => ({ sessions: st.sessions.some((x) => x.id === s.id) ? st.sessions : [s, ...st.sessions] }));
    await get().selectSession(s.id);
    await get().loadSessions(workspaceId);
    return s.id;
  },

  send: async (text, attachments, images = []) => {
    const id = get().activeSessionId;
    if (!id) throw new Error("no active session");
    await get().sendTo(id, text, attachments, images);
  },

  sendTo: async (sessionId, text, attachments, images = []) => {
    // Sending by hand releases a held queue.
    set((s) => ({ queueHeld: { ...s.queueHeld, [sessionId]: false } }));
    if (sessionId === get().activeSessionId) set({ lastError: null, status: "thinking" });
    running.add(sessionId);
    try {
      await sendWhenFree(sessionId, text, attachments, images);
    } catch (e) {
      running.delete(sessionId);
      if (sessionId === get().activeSessionId) set({ status: "idle" });
      throw e;
    }
  },

  // When a turn ends: steering messages it never picked up go back to
  // the front of the queue, then the next queued message is sent (unless
  // the turn was stopped or failed).
  turnEnded: (sessionId, hold = false) => {
    running.delete(sessionId);
    const leftover = get().steering[sessionId] ?? [];
    const queue: QueuedMessage[] = [
      ...leftover.map((m) => ({ id: m.id, text: m.text, attachments: [], images: [] })),
      ...(get().queued[sessionId] ?? []),
    ];
    const held = hold || get().queueHeld[sessionId] === true;
    const next = held ? undefined : queue[0];
    const rest = next ? queue.slice(1) : queue;
    set((s) => ({
      steering: { ...s.steering, [sessionId]: [] },
      queued: { ...s.queued, [sessionId]: rest },
      queueHeld: { ...s.queueHeld, [sessionId]: held && queue.length > 0 },
    }));
    if (next) void get().sendTo(sessionId, next.text, next.attachments, next.images).catch((err) => {
      set((s) => ({
        lastError: sessionId === s.activeSessionId ? String(err) : s.lastError,
        queued: { ...s.queued, [sessionId]: [next, ...(s.queued[sessionId] ?? [])] },
        queueHeld: { ...s.queueHeld, [sessionId]: true },
      }));
    });
  },

  enqueue: (text, attachments, images = []) => {
    const id = get().activeSessionId;
    if (!id) return;
    set((s) => ({
      queued: { ...s.queued, [id]: [...(s.queued[id] ?? []), { id: localId(), text, attachments, images }] },
    }));
  },

  removeQueued: (msgId) => {
    const id = get().activeSessionId;
    if (!id) return;
    set((s) => ({
      queued: { ...s.queued, [id]: (s.queued[id] ?? []).filter((m) => m.id !== msgId) },
    }));
  },

  steer: async (text) => {
    const id = get().activeSessionId;
    if (!id) return;
    const accepted = running.has(id) ? await api.steerMessage(id, text) : null;
    if (!accepted) {
      // The turn ended meanwhile: it becomes the next message instead.
      get().enqueue(text, []);
      if (!running.has(id)) get().turnEnded(id);
      return;
    }
    // Already injected (its message can arrive before this reply).
    if (seenUserMessages.has(accepted)) return;
    set((s) => ({ steering: { ...s.steering, [id]: [...(s.steering[id] ?? []), { id: accepted, text }] } }));
    // Accepted just as the turn was ending: the runtime dropped it.
    if (!running.has(id)) get().turnEnded(id);
  },

  steerQueued: async (msgId) => {
    const id = get().activeSessionId;
    const item = id ? get().queued[id]?.find((m) => m.id === msgId) : undefined;
    if (!id || !item) return;
    get().removeQueued(msgId);
    await get().steer(item.text);
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
        usage: null,
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
