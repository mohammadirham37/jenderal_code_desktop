/** Git panel: status, staging, commit, branches, log, diff view. */
import { useCallback, useEffect, useState } from "react";
import {
  Check,
  GitBranch,
  GitCommitHorizontal,
  Plus,
  RotateCcw,
  RefreshCw,
  FileDiff,
} from "lucide-react";
import { api, type BranchInfo, type CommitInfo, type FileStatus } from "@/lib/ipc";
import { useEditor } from "@/stores/editor";
import { Button, EmptyState, IconButton, Spinner, inputClass } from "@/components/ui";
import { formatRelative } from "@/lib/format";

const KIND_LABEL: Record<string, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  copied: "C",
  untracked: "??",
  ignored: "!",
  unmodified: " ",
  other: "~",
};

export function GitPanel() {
  const [status, setStatus] = useState<FileStatus[]>([]);
  const [branch, setBranch] = useState<string | null>(null);
  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [log, setLog] = useState<CommitInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const openDiff = useEditor((s) => s.openDiff);

  const refresh = useCallback(async () => {
    try {
      const [st, br, lg] = await Promise.all([
        api.gitStatus().catch(() => [] as FileStatus[]),
        api.gitCurrentBranch().catch(() => null),
        api.gitLog(15).catch(() => [] as CommitInfo[]),
      ]);
      setStatus(st);
      setBranch(br);
      setBranches(await api.gitBranches().catch(() => [] as BranchInfo[]));
      setLog(lg);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = setInterval(() => void refresh(), 10_000);
    return () => clearInterval(t);
  }, [refresh]);

  const toggle = (path: string) => {
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const stageSelected = async () => {
    const paths = Array.from(selected);
    if (!paths.length) return;
    await api.gitStage(paths);
    setSelected(new Set());
    await refresh();
  };

  const unstageSelected = async () => {
    const paths = Array.from(selected);
    if (!paths.length) return;
    await api.gitUnstage(paths);
    setSelected(new Set());
    await refresh();
  };

  const discard = async (path: string) => {
    if (!window.confirm(`Discard changes in ${path}? This cannot be undone.`)) return;
    await api.gitDiscard(path);
    await refresh();
  };

  const commit = async () => {
    if (!message.trim()) return;
    setBusy(true);
    try {
      // Commit staged changes; if nothing staged, stage selections first.
      const staged = status.some((f) => f.index !== "unmodified" && f.index !== "untracked");
      if (!staged) {
        const toStage = selected.size ? Array.from(selected) : [];
        if (!toStage.length) {
          setError("Stage changes or select files to commit.");
          return;
        }
        await api.gitStage(toStage);
      }
      await api.gitCommit(message.trim());
      setMessage("");
      setSelected(new Set());
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const viewDiff = async (path: string) => {
    const staged = status.find((f) => f.path === path)?.index !== "untracked";
    const diff = await api.gitDiff(path, staged);
    await openDiff(`git:${path}`, path, diff);
  };

  const viewAllDiff = async () => {
    const diff = await api.gitDiffAll();
    await openDiff("git:all", "Working tree vs HEAD", diff);
  };

  if (error && status.length === 0 && branch === null) {
    return <EmptyState title="Not a git repository" hint={error ?? undefined} />;
  }

  const grouped = {
    staged: status.filter((f) => f.index !== "unmodified" && f.index !== "untracked"),
    unstaged: status.filter((f) => f.index === "unmodified" && f.worktree !== "unmodified"),
    untracked: status.filter((f) => f.index === "untracked"),
  };

  return (
    <div className="flex h-full min-h-0">
      {/* Left: changes list */}
      <div className="flex w-1/2 min-w-0 flex-col border-r border-neutral-200 dark:border-neutral-800">
        <div className="flex items-center justify-between border-b border-neutral-200 px-2 py-1.5 dark:border-neutral-800">
          <span className="flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-neutral-500">
            <GitBranch size={11} /> {branch ?? "detached"}
          </span>
          <div className="flex items-center gap-0.5">
            <IconButton title="View full diff" className="h-6 w-6" onClick={() => void viewAllDiff()}>
              <FileDiff size={12} />
            </IconButton>
            <IconButton title="Refresh" className="h-6 w-6" onClick={() => void refresh()}>
              <RefreshCw size={12} />
            </IconButton>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-1">
          {status.length === 0 && (
            <div className="p-4 text-center text-xs text-neutral-500">Working tree clean</div>
          )}
          {(
            [
              ["Staged changes", grouped.staged],
              ["Changes", grouped.unstaged],
              ["Untracked", grouped.untracked],
            ] as const
          ).map(([label, files]) =>
            files.length ? (
              <div key={label} className="mb-2">
                <div className="px-1.5 py-1 text-[10px] font-semibold uppercase text-neutral-500">
                  {label} ({files.length})
                </div>
                {files.map((f) => (
                  <div
                    key={f.path}
                    className="group flex cursor-pointer items-center gap-1.5 rounded px-1.5 py-1 text-xs hover:bg-neutral-200/60 dark:hover:bg-neutral-800/60"
                    onClick={() => toggle(f.path)}
                  >
                    <span
                      className={`w-4 text-center font-mono text-[10px] ${
                        f.index === "untracked"
                          ? "text-neutral-400"
                          : f.index === "added" || f.worktree === "added"
                            ? "text-emerald-500"
                            : "text-amber-500"
                      }`}
                    >
                      {f.index === "untracked" ? "??" : KIND_LABEL[f.index] ?? "M"}
                    </span>
                    <span
                      className="min-w-0 flex-1 truncate"
                      onClick={(e) => {
                        e.stopPropagation();
                        void viewDiff(f.path);
                      }}
                    >
                      {f.path}
                    </span>
                    <button
                      title="Discard changes"
                      className="hidden rounded p-0.5 text-neutral-500 hover:text-red-500 group-hover:block"
                      onClick={(e) => {
                        e.stopPropagation();
                        void discard(f.path);
                      }}
                    >
                      <RotateCcw size={11} />
                    </button>
                  </div>
                ))}
              </div>
            ) : null,
          )}
        </div>
        <div className="border-t border-neutral-200 p-2 dark:border-neutral-800">
          <div className="mb-2 flex gap-1">
            <Button size="sm" onClick={stageSelected} disabled={!selected.size}>
              <Plus size={11} /> Stage
            </Button>
            <Button size="sm" onClick={unstageSelected} disabled={!selected.size}>
              Unstage
            </Button>
          </div>
          <div className="flex gap-1">
            <input
              className={inputClass}
              placeholder="Commit message…"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void commit()}
            />
            <Button variant="primary" size="sm" onClick={() => void commit()} disabled={busy || !message.trim()}>
              {busy ? <Spinner /> : <Check size={11} />} Commit
            </Button>
          </div>
        </div>
      </div>

      {/* Right: branches & log */}
      <div className="flex w-1/2 min-w-0 flex-col">
        <div className="border-b border-neutral-200 px-2 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-neutral-500 dark:border-neutral-800">
          Branches
        </div>
        <div className="max-h-28 overflow-y-auto p-1">
          {branches.map((b) => (
            <button
              key={b.name}
              onClick={async () => {
                if (!b.current && window.confirm(`Checkout branch ${b.name}?`)) {
                  await api.gitCheckoutBranch(b.name);
                  await refresh();
                }
              }}
              className={`flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs hover:bg-neutral-200/60 dark:hover:bg-neutral-800/60 ${
                b.current ? "text-indigo-400" : ""
              }`}
            >
              <GitBranch size={11} />
              {b.name}
              {b.current && <span className="ml-auto text-[9px] uppercase">current</span>}
            </button>
          ))}
        </div>
        <div className="border-y border-neutral-200 px-2 py-1.5 text-[10px] font-semibold uppercase tracking-wider text-neutral-500 dark:border-neutral-800">
          Recent commits
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-1">
          {log.map((c) => (
            <div key={c.hash} className="flex items-start gap-1.5 px-1.5 py-1 text-xs">
              <GitCommitHorizontal size={12} className="mt-0.5 shrink-0 text-neutral-500" />
              <div className="min-w-0">
                <div className="truncate">{c.subject}</div>
                <div className="text-[10px] text-neutral-500">
                  {c.short_hash} · {c.author} · {formatRelative(c.timestamp * 1000)}
                </div>
              </div>
            </div>
          ))}
          {log.length === 0 && <div className="p-3 text-center text-xs text-neutral-500">No commits</div>}
        </div>
      </div>
    </div>
  );
}
