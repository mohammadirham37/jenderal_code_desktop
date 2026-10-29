/** Live list of changed files in the workspace.
 *
 *  Git repos: `git status` against HEAD, refreshed whenever files change on
 *  disk (watcher), an agent tool finishes, the agent goes idle, or the window
 *  regains focus — so changes made by the agent, by terminal commands
 *  (artisan, npm, …) and by hand all show up. Files the agent touched in
 *  this conversation are marked.
 *  Non-git folders: the agent's edits plus files changed on disk since the
 *  panel opened. */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bot, FileCode2, FileDiff, FileMinus, FilePlus, RefreshCw } from "lucide-react";
import { api, events, type FileStatus } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { useChat } from "@/stores/chat";
import { useEditor } from "@/stores/editor";
import { EmptyState, IconButton } from "@/components/ui";

type Kind = "added" | "modified" | "deleted" | "renamed" | "untracked";

interface Change {
  path: string;
  kind: Kind;
  staged: boolean;
  byAgent: boolean;
}

const AGENT_WRITE_TOOLS: Record<string, Kind> = { fs_write: "modified", fs_edit: "modified", fs_delete: "deleted" };

const TONE: Record<Kind, string> = {
  added: "text-emerald-500",
  untracked: "text-emerald-500",
  modified: "text-amber-500",
  renamed: "text-sky-500",
  deleted: "text-red-500",
};
const LETTER: Record<Kind, string> = { added: "A", untracked: "U", modified: "M", renamed: "R", deleted: "D" };

function kindOf(s: FileStatus): Kind {
  const code = s.worktree.trim() || s.index.trim();
  if (s.index === "?" || code === "?") return "untracked";
  if (code === "A") return "added";
  if (code === "D") return "deleted";
  if (code === "R") return "renamed";
  return "modified";
}

export function ChangesPanel({ onOpen }: { onOpen: () => void }) {
  const workspace = useApp((s) => s.workspace);
  const toolRuns = useChat((s) => s.toolRuns);
  const status = useChat((s) => s.status);
  const openFile = useEditor((s) => s.openFile);
  const openDiff = useEditor((s) => s.openDiff);
  const isGit = !!workspace?.is_git_repo;

  const [gitFiles, setGitFiles] = useState<FileStatus[] | null>(null);
  const [diskChanges, setDiskChanges] = useState<string[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inFlight = useRef(false);
  const again = useRef(false);

  // Agent edits in this conversation (successful runs only).
  const agentChanges = useMemo(() => {
    const byPath = new Map<string, Kind>();
    for (const r of toolRuns) {
      const kind = AGENT_WRITE_TOOLS[r.tool_name];
      const path = typeof r.args?.path === "string" ? r.args.path.replace(/^\.\//, "") : null;
      if (kind && path && r.status === "success") byPath.set(path, kind);
    }
    return byPath;
  }, [toolRuns]);

  const refresh = useCallback(async () => {
    if (!isGit) return;
    if (inFlight.current) {
      again.current = true; // coalesce bursts into one follow-up refresh
      return;
    }
    inFlight.current = true;
    setRefreshing(true);
    try {
      setGitFiles(await api.gitStatus());
      setError(null);
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    } finally {
      inFlight.current = false;
      setRefreshing(false);
      if (again.current) {
        again.current = false;
        void refresh();
      }
    }
  }, [isGit]);

  const scheduleRefresh = useCallback(() => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => void refresh(), 250);
  }, [refresh]);

  // Initial load + reload when switching projects.
  useEffect(() => {
    setGitFiles(null);
    setDiskChanges([]);
    void refresh();
  }, [workspace?.id, refresh]);

  // Files changed on disk (agent, terminal, editor, external tools).
  useEffect(() => {
    const root = workspace?.root_path.replace(/\/+$/, "") ?? "";
    const un = events.fsChanged((e) => {
      if (isGit) {
        scheduleRefresh();
        return;
      }
      const rel = e.paths
        .map((p) => (p.startsWith(`${root}/`) ? p.slice(root.length + 1) : p))
        .filter((p) => p && !p.startsWith("/"));
      if (rel.length) setDiskChanges((cur) => [...new Set([...rel, ...cur])].slice(0, 200));
    });
    return () => {
      void un.then((f) => f());
      if (timer.current) clearTimeout(timer.current);
    };
  }, [workspace?.root_path, isGit, scheduleRefresh]);

  // Agent finished a tool, or the turn ended (catches changes the watcher
  // might coalesce, e.g. many files written by one command).
  const finishedRuns = toolRuns.filter((r) => r.status === "success" || r.status === "error").length;
  useEffect(() => {
    scheduleRefresh();
  }, [finishedRuns, scheduleRefresh]);
  useEffect(() => {
    if (status === "idle") scheduleRefresh();
  }, [status, scheduleRefresh]);

  useEffect(() => {
    const onFocus = () => scheduleRefresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [scheduleRefresh]);

  const changes: Change[] = useMemo(() => {
    if (isGit && gitFiles) {
      return gitFiles
        .map((s) => ({
          path: s.path,
          kind: kindOf(s),
          staged: s.index.trim() !== "" && s.index !== "?",
          byAgent: agentChanges.has(s.path),
        }))
        .sort((a, b) => Number(b.byAgent) - Number(a.byAgent) || a.path.localeCompare(b.path));
    }
    const out = new Map<string, Change>();
    for (const [path, kind] of agentChanges) out.set(path, { path, kind, staged: false, byAgent: true });
    for (const path of diskChanges) if (!out.has(path)) out.set(path, { path, kind: "modified", staged: false, byAgent: false });
    return [...out.values()];
  }, [isGit, gitFiles, agentChanges, diskChanges]);

  const open = async (c: Change) => {
    try {
      if (isGit) {
        const diff = await api.gitDiffAll(c.path);
        if (diff.trim()) {
          await openDiff(`changes:${c.path}`, c.path, diff);
          onOpen();
          return;
        }
      }
      if (c.kind !== "deleted") {
        await openFile(c.path);
        onOpen();
      }
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  const agentCount = changes.filter((c) => c.byAgent).length;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-neutral-200 px-2.5 text-[10px] font-semibold uppercase tracking-wider text-neutral-500 dark:border-neutral-800">
        <span className="relative flex h-1.5 w-1.5" title="Updates live">
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />
          <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-emerald-500" />
        </span>
        <span>
          {changes.length} changed{agentCount > 0 && ` · ${agentCount} by agent`}
        </span>
        {isGit && (
          <IconButton className="ml-auto h-6 w-6" title="Refresh" onClick={() => void refresh()}>
            <RefreshCw size={11} className={refreshing ? "animate-spin" : ""} />
          </IconButton>
        )}
      </div>
      {error && <div className="shrink-0 bg-red-500/10 px-3 py-1 text-[11px] text-red-400">{error}</div>}
      {changes.length === 0 ? (
        <EmptyState
          icon={<FileDiff size={26} />}
          title={isGit && gitFiles === null ? "Loading changes…" : "No changes"}
          hint={
            isGit
              ? "Uncommitted changes in this project appear here as they happen — from the agent, the terminal, or your own edits."
              : "Files changed by the agent or on disk appear here as they happen."
          }
        />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {changes.map((c) => {
            const Icon = c.kind === "deleted" ? FileMinus : c.kind === "added" || c.kind === "untracked" ? FilePlus : FileCode2;
            return (
              <button
                key={c.path}
                onClick={() => void open(c)}
                className="group flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-neutral-100 dark:hover:bg-neutral-800/60"
                title={`${c.path} — click to view ${isGit ? "diff" : "file"}`}
              >
                <Icon size={13} className={`shrink-0 ${TONE[c.kind]}`} />
                <span
                  className={`min-w-0 flex-1 truncate font-mono text-[11px] ${c.kind === "deleted" ? "line-through opacity-70" : ""}`}
                >
                  {c.path}
                </span>
                {c.byAgent && (
                  <span title="Changed by the agent in this conversation" className="shrink-0 text-indigo-400">
                    <Bot size={11} />
                  </span>
                )}
                {c.staged && <span className="shrink-0 text-[9px] uppercase text-neutral-500">staged</span>}
                <span className={`w-3 shrink-0 text-center font-mono text-[10px] font-bold ${TONE[c.kind]}`}>{LETTER[c.kind]}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
