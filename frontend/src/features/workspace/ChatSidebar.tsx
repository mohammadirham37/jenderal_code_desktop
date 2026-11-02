/** Chat layout sidebar: new conversation, the conversation history (in a
 *  modal), and recently opened projects for quick switching. */
import { useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { FolderGit2, FolderOpen, FolderPlus, History, MessageSquare, MoreHorizontal, Pencil, Plus, Search, Trash2, X } from "lucide-react";
import { useApp } from "@/stores/app";
import { useChat } from "@/stores/chat";
import { IconButton, Modal } from "@/components/ui";
import { formatRelative } from "@/lib/format";
import type { WorkspaceInfo } from "@/lib/ipc";
import { confirmAction } from "@/lib/confirm";
import { useCenterTabs } from "@/stores/centerTabs";

export function ChatSidebar() {
  const workspace = useApp((s) => s.workspace);
  const [historyOpen, setHistoryOpen] = useState(false);

  return (
    <div className="flex h-full flex-col bg-neutral-100/70 dark:bg-neutral-925">
      <div className="space-y-1.5 p-2">
        <button
          onClick={() => workspace && void useChat.getState().newSession(workspace.id)}
          className="flex w-full items-center justify-between rounded-[5px] bg-accent-600 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-accent-700 dark:bg-accent-400 dark:text-neutral-950 dark:hover:bg-accent-300"
        >
          <span className="flex items-center gap-1.5">
            <Plus size={13} /> New conversation
          </span>
          <kbd className="font-mono text-[10px] opacity-60">⌘K</kbd>
        </button>
        <button
          onClick={() => setHistoryOpen(true)}
          className="flex w-full items-center gap-1.5 rounded-[5px] border border-neutral-200 bg-white px-3 py-1.5 text-xs text-neutral-700 transition-colors hover:border-neutral-300 dark:border-neutral-800 dark:bg-neutral-950 dark:text-neutral-300 dark:hover:border-neutral-700"
        >
          <History size={13} className="text-neutral-500" /> Conversation history
        </button>
      </div>
      <ProjectList />
      <HistoryModal open={historyOpen} onClose={() => setHistoryOpen(false)} />
    </div>
  );
}

/** Recently opened projects, newest first; the active one is marked. */
function ProjectList() {
  const { workspace, recent } = useApp(useShallow((s) => ({ workspace: s.workspace, recent: s.recent })));
  const [switching, setSwitching] = useState<string | null>(null);
  // Terminal / agent-CLI tabs still running in the other projects.
  const parked = useCenterTabs((s) => s.parked);
  const running = (id: string) => parked[id]?.tabs.filter((t) => !t.exited) ?? [];
  const [menu, setMenu] = useState<{ w: WorkspaceInfo; x: number; y: number } | null>(null);

  /** Take a project off the list (its chat history stays; opening the
   *  folder again brings it back). The open project is closed first. */
  const remove = async (w: WorkspaceInfo) => {
    setMenu(null);
    const isOpen = w.id === workspace?.id;
    const live = isOpen ? useCenterTabs.getState().tabs.filter((t) => !t.exited) : running(w.id);
    const parts = [
      isOpen ? `Close ${w.name} and remove it from the list?` : `Remove ${w.name} from the list?`,
      live.length ? `Still running: ${live.map((t) => t.title).join(", ")}. They will be stopped.` : "",
      "Its files and chat history are kept; open the folder again to bring it back.",
    ];
    if ((isOpen || live.length) && !(await confirmAction(parts.filter(Boolean).join("\n\n")))) return;
    if (isOpen) await useApp.getState().closeWorkspace();
    await useApp.getState().removeRecent(w.id);
  };
  const projects = useMemo(() => [...recent].sort((a, b) => b.last_opened_at - a.last_opened_at), [recent]);

  const open = async (w: WorkspaceInfo) => {
    if (w.id === workspace?.id || switching) return;
    setSwitching(w.id);
    try {
      await useApp.getState().openWorkspacePath(w.root_path);
    } finally {
      setSwitching(null);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between px-3 pb-1 pt-2">
        <span className="eyebrow">Projects</span>
        <IconButton title="Open folder…" className="h-6 w-6" onClick={() => void useApp.getState().openFolderPicker()}>
          <FolderPlus size={13} />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
        {projects.length === 0 && <div className="px-2 py-6 text-center text-[11px] text-neutral-500">No projects yet</div>}
        {projects.map((w) => {
          const active = w.id === workspace?.id;
          const Icon = w.is_git_repo ? FolderGit2 : FolderOpen;
          return (
            <div
              key={w.id}
              onClick={() => void open(w)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ w, x: e.clientX, y: e.clientY });
              }}
              title={w.root_path}
              className={`group relative mb-px cursor-pointer rounded-[4px] px-2.5 py-2 transition-colors ${
                active
                  ? "bg-white shadow-[inset_0_0_0_1px_var(--color-neutral-200)] dark:bg-neutral-850 dark:shadow-[inset_0_0_0_1px_var(--color-neutral-800)]"
                  : "hover:bg-neutral-200/60 dark:hover:bg-neutral-800/50"
              } ${switching === w.id ? "animate-pulse" : ""}`}
            >
              <div className="flex items-center gap-1.5">
                <Icon size={12} className={`shrink-0 ${active ? "text-accent-600 dark:text-accent-400" : "text-neutral-400"}`} />
                <span className={`min-w-0 flex-1 truncate text-xs ${active ? "font-medium" : ""}`}>{w.name}</span>
                {!active && running(w.id).length > 0 && (
                  <span
                    title={`Still running: ${running(w.id).map((t) => t.title).join(", ")}`}
                    className="flex shrink-0 items-center gap-1 rounded-[3px] bg-accent-500/10 px-1 font-mono text-[10px] text-accent-700 group-hover:hidden dark:text-accent-300"
                  >
                    <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-accent-500" />
                    {running(w.id).length}
                  </span>
                )}
                <button
                  title="Project actions"
                  aria-label={`Actions for ${w.name}`}
                  className={`shrink-0 rounded p-0.5 text-neutral-500 hover:bg-neutral-300/60 dark:hover:bg-neutral-700 ${
                    menu?.w.id === w.id ? "block" : "hidden group-hover:block"
                  }`}
                  onClick={(e) => {
                    e.stopPropagation();
                    const r = e.currentTarget.getBoundingClientRect();
                    setMenu(menu?.w.id === w.id ? null : { w, x: r.right - 176, y: r.bottom + 4 });
                  }}
                >
                  <MoreHorizontal size={12} />
                </button>
              </div>
              <div className="mt-0.5 truncate pl-[18px] font-mono text-[10px] text-neutral-500">{shortPath(w.root_path)}</div>
              {active && <span className="absolute inset-y-2 left-0 w-[2px] bg-accent-500" />}
            </div>
          );
        })}
      </div>
      {menu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
          <div
            className="animate-rise fixed z-50 w-44 overflow-hidden rounded-[6px] border border-neutral-200 bg-white py-1 shadow-xl dark:border-neutral-700 dark:bg-neutral-900"
            style={{ left: Math.max(8, menu.x), top: Math.min(menu.y, window.innerHeight - 60) }}
          >
            <button
              onClick={() => void remove(menu.w)}
              className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-red-600 hover:bg-red-500/10 dark:text-red-400"
            >
              <Trash2 size={12} /> Remove from list
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/** `/Users/me/code/app` -> `~/code/app`. */
function shortPath(path: string): string {
  return path.replace(/^\/(Users|home)\/[^/]+/, "~");
}

/** Conversations of the current project: search, open, rename, delete. */
function HistoryModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const workspace = useApp((s) => s.workspace);
  const { sessions, activeSessionId } = useChat(
    useShallow((s) => ({ sessions: s.sessions, activeSessionId: s.activeSessionId })),
  );
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !editing) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, editing, onClose]);

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sessions
      .filter((s) => s.workspace_id === workspace?.id)
      .filter((s) => !q || s.title.toLowerCase().includes(q))
      .sort((a, b) => b.updated_at - a.updated_at);
  }, [sessions, workspace?.id, query]);

  const commitRename = async (id: string) => {
    const title = draft.trim();
    setEditing(null);
    if (title) await useChat.getState().rename(id, title);
  };

  return (
    <Modal open={open} title={`Conversations · ${workspace?.name ?? ""}`} onClose={onClose}>
      <div className="relative mb-3">
        <Search size={12} className="absolute left-2.5 top-2 text-neutral-400" />
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search conversations"
          className="w-full rounded-[5px] border border-neutral-200 bg-white py-1.5 pl-7 pr-2 text-xs focus:border-accent-500 focus:outline-none dark:border-neutral-800 dark:bg-neutral-950"
        />
      </div>
      {list.length === 0 && (
        <div className="py-6 text-center text-[11px] text-neutral-500">
          {query ? "No matching conversations" : "No conversations yet"}
        </div>
      )}
      <div className="-mx-1">
        {list.map((s) => {
          const active = s.id === activeSessionId;
          return (
            <div
              key={s.id}
              onClick={() => {
                if (editing === s.id) return;
                if (!active) void useChat.getState().selectSession(s.id);
                onClose();
              }}
              className={`group relative mb-px cursor-pointer rounded-[4px] px-2.5 py-2 transition-colors ${
                active ? "bg-accent-500/10" : "hover:bg-neutral-100 dark:hover:bg-neutral-800/60"
              }`}
            >
              {editing === s.id ? (
                <input
                  autoFocus
                  value={draft}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => void commitRename(s.id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void commitRename(s.id);
                    if (e.key === "Escape") setEditing(null);
                  }}
                  className="w-full rounded border border-accent-500/50 bg-transparent px-1 text-xs focus:outline-none"
                />
              ) : (
                <div className="flex items-center gap-1.5">
                  <MessageSquare size={12} className={`shrink-0 ${active ? "text-accent-600 dark:text-accent-400" : "text-neutral-400"}`} />
                  <span className={`min-w-0 flex-1 truncate text-xs ${active ? "font-medium" : ""}`}>{s.title}</span>
                  <span className="shrink-0 font-mono text-[10px] text-neutral-500 group-hover:hidden">
                    {formatRelative(s.updated_at)} · {s.message_count} msg
                  </span>
                  <span className="hidden shrink-0 items-center group-hover:flex">
                    <button
                      title="Rename"
                      className="rounded p-0.5 text-neutral-500 hover:bg-neutral-300/60 dark:hover:bg-neutral-700"
                      onClick={(e) => {
                        e.stopPropagation();
                        setDraft(s.title);
                        setEditing(s.id);
                      }}
                    >
                      <Pencil size={11} />
                    </button>
                    <button
                      title="Delete"
                      className="rounded p-0.5 text-neutral-500 hover:bg-red-500/20 hover:text-red-500"
                      onClick={async (e) => {
                        e.stopPropagation();
                        if (await confirmAction(`Delete conversation "${s.title}"?`)) void useChat.getState().removeSession(s.id);
                      }}
                    >
                      <Trash2 size={11} />
                    </button>
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </Modal>
  );
}
