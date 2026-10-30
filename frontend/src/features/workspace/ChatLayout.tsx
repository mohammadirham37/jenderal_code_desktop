/** Chat-first layout: sessions sidebar | centre column (Chat plus terminal
 *  and agent-CLI tabs) | collapsible context panel (changed files, explorer,
 *  editor, terminal, git). */
import { useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import {
  FileCode2,
  FileDiff,
  FolderTree,
  GitBranch,
  Globe,
  MessageSquare,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Plus,
  Search,
  Terminal as TerminalIcon,
  Trash2,
} from "lucide-react";
import { BrowserPanel } from "@/features/browser/BrowserPanel";
import { FileExplorer } from "./FileExplorer";
import { ChangesPanel } from "./ChangesPanel";
import { CenterColumn } from "./CenterColumn";
import { LazyEditorPane, LazyGitPanel, LazyTerminalPanel, PanelSuspense } from "./lazy";
import { useApp } from "@/stores/app";
import { useChat } from "@/stores/chat";
import { useEditor } from "@/stores/editor";
import { IconButton } from "@/components/ui";
import { formatRelative } from "@/lib/format";
import { useResizable } from "./useResizable";

type ContextTab = "changes" | "files" | "editor" | "browser" | "terminal" | "git";

const CONTEXT_TABS: { id: ContextTab; label: string; icon: typeof FileDiff }[] = [
  { id: "changes", label: "Changes", icon: FileDiff },
  { id: "files", label: "Files", icon: FolderTree },
  { id: "editor", label: "Editor", icon: FileCode2 },
  { id: "browser", label: "Browser", icon: Globe },
  { id: "terminal", label: "Terminal", icon: TerminalIcon },
  { id: "git", label: "Git", icon: GitBranch },
];

const PANEL_OPEN_KEY = "jcode:chat-context-open";

export function ChatLayout() {
  const [contextOpen, setContextOpen] = useState(() => localStorage.getItem(PANEL_OPEN_KEY) !== "0");
  const [tab, setTab] = useState<ContextTab>("changes");
  // Panels are mounted on first visit and then kept alive (hidden), so the
  // terminal's shell sessions survive tab switches.
  const [visited, setVisited] = useState<Set<ContextTab>>(() => new Set(["changes"]));
  const sidebar = useResizable("jcode:chat-sidebar-width", 240, 180, 380, "right");
  const context = useResizable("jcode:chat-context-width", 440, 300, 900, "left");

  const showTab = (t: ContextTab) => {
    setTab(t);
    setVisited((v) => (v.has(t) ? v : new Set(v).add(t)));
    setContextOpen(true);
  };

  useEffect(() => {
    localStorage.setItem(PANEL_OPEN_KEY, contextOpen ? "1" : "0");
  }, [contextOpen]);

  // Opening a file or diff from the chat (or explorer) reveals the editor.
  const activeKey = useEditor((s) => s.activeKey);
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    if (activeKey) showTab("editor");
  }, [activeKey]);

  return (
    <div className="flex min-h-0 flex-1">
      <div style={{ width: sidebar.size }} className="min-h-0 shrink-0 border-r border-neutral-200 dark:border-neutral-800">
        <SessionSidebar />
      </div>
      <div {...sidebar.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />

      <div className="relative min-w-0 flex-1">
        <CenterColumn
          trailing={
            <IconButton
              title={contextOpen ? "Hide context panel" : "Show context panel"}
              onClick={() => setContextOpen(!contextOpen)}
            >
              {contextOpen ? <PanelRightClose size={14} /> : <PanelRightOpen size={14} />}
            </IconButton>
          }
        />
      </div>

      {contextOpen && (
        <div {...context.handleProps} className="w-1 shrink-0 cursor-col-resize hover:bg-accent-500/40" />
      )}
      <div
        style={{ width: context.size }}
        className={`min-h-0 shrink-0 flex-col border-l border-neutral-200 dark:border-neutral-800 ${
          contextOpen ? "flex" : "hidden"
        }`}
      >
        <div className="flex h-10 shrink-0 items-stretch gap-3 overflow-x-auto border-b border-neutral-200 px-3 [scrollbar-width:none] dark:border-neutral-800">
          {CONTEXT_TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => showTab(id)}
              title={label}
              className={`-mb-px flex shrink-0 items-center gap-1.5 border-b-2 font-mono text-[10.5px] tracking-wide uppercase transition-colors ${
                tab === id
                  ? "border-accent-500 text-neutral-900 dark:text-neutral-100"
                  : "border-transparent text-neutral-500 hover:text-neutral-800 dark:hover:text-neutral-200"
              }`}
            >
              <Icon size={12} /> {label}
            </button>
          ))}
        </div>
        <div className="relative min-h-0 flex-1">
          {[...visited].map((t) => (
            <div key={t} className={`absolute inset-0 ${t === tab ? "block" : "hidden"} ${darkPanel(t)}`}>
              <PanelSuspense>
                {t === "changes" && <ChangesPanel onOpen={() => showTab("editor")} />}
                {t === "files" && <FileExplorer />}
                {t === "editor" && <LazyEditorPane />}
                {t === "browser" && <BrowserPanel id="chat" />}
                {t === "terminal" && <LazyTerminalPanel />}
                {t === "git" && <LazyGitPanel />}
              </PanelSuspense>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function darkPanel(t: ContextTab) {
  return t === "terminal" || t === "git" ? "bg-neutral-950 text-neutral-200" : "";
}

/** Sessions of the current workspace, newest first. */
function SessionSidebar() {
  const workspace = useApp((s) => s.workspace);
  const { sessions, activeSessionId } = useChat(
    useShallow((s) => ({ sessions: s.sessions, activeSessionId: s.activeSessionId })),
  );
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

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
    <div className="flex h-full flex-col bg-neutral-100/70 dark:bg-neutral-925">
      <div className="p-2">
        <button
          onClick={() => workspace && void useChat.getState().newSession(workspace.id)}
          className="flex w-full items-center justify-between rounded-[5px] bg-accent-600 px-3 py-2 text-xs font-medium text-white transition-colors hover:bg-accent-700 dark:bg-accent-400 dark:text-neutral-950 dark:hover:bg-accent-300"
        >
          <span className="flex items-center gap-1.5">
            <Plus size={13} /> New conversation
          </span>
          <kbd className="font-mono text-[10px] opacity-60">⌘K</kbd>
        </button>
        <div className="relative mt-2">
          <Search size={12} className="absolute left-2.5 top-2 text-neutral-400" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search conversations"
            className="w-full rounded-[5px] border border-neutral-200 bg-white py-1.5 pl-7 pr-2 text-[11px] focus:border-accent-500 focus:outline-none dark:border-neutral-800 dark:bg-neutral-950"
          />
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
        {list.length === 0 && (
          <div className="px-2 py-6 text-center text-[11px] text-neutral-500">
            {query ? "No matching conversations" : "No conversations yet"}
          </div>
        )}
        {list.map((s) => {
          const active = s.id === activeSessionId;
          return (
            <div
              key={s.id}
              onClick={() => !active && void useChat.getState().selectSession(s.id)}
              className={`group relative mb-px cursor-pointer rounded-[4px] px-2.5 py-2 transition-colors ${
                active
                  ? "bg-white shadow-[inset_0_0_0_1px_var(--color-neutral-200)] dark:bg-neutral-850 dark:shadow-[inset_0_0_0_1px_var(--color-neutral-800)]"
                  : "hover:bg-neutral-200/60 dark:hover:bg-neutral-800/50"
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
                      onClick={(e) => {
                        e.stopPropagation();
                        if (window.confirm(`Delete conversation "${s.title}"?`)) void useChat.getState().removeSession(s.id);
                      }}
                    >
                      <Trash2 size={11} />
                    </button>
                  </span>
                </div>
              )}
              {active && <span className="absolute inset-y-2 left-0 w-[2px] bg-accent-500" />}
              <div className="mt-0.5 pl-[18px] font-mono text-[10px] text-neutral-500">
                {formatRelative(s.updated_at)} · {s.message_count} msg
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
