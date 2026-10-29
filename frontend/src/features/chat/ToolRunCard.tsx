/** Tool-run card: shows agent tool activity with status and results. */
import { useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  FileEdit,
  FilePlus,
  FileSearch,
  FolderTree,
  Loader2,
  Search,
  ShieldAlert,
  Terminal,
  Trash2,
  GitBranch,
  X,
} from "lucide-react";
import type { ToolRunRecord } from "@/lib/ipc";
import { Badge } from "@/components/ui";

const TOOL_META: Record<string, { icon: typeof FileEdit; label: string }> = {
  fs_read: { icon: FileSearch, label: "Read file" },
  fs_write: { icon: FilePlus, label: "Write file" },
  fs_edit: { icon: FileEdit, label: "Edit file" },
  fs_delete: { icon: Trash2, label: "Delete" },
  fs_list: { icon: FolderTree, label: "List directory" },
  fs_search: { icon: Search, label: "Search" },
  glob_files: { icon: Search, label: "Glob files" },
  terminal_run: { icon: Terminal, label: "Run command" },
  git_status: { icon: GitBranch, label: "Git status" },
  git_diff: { icon: GitBranch, label: "Git diff" },
  git_log: { icon: GitBranch, label: "Git log" },
};

export function statusVisual(status: ToolRunRecord["status"]) {
  switch (status) {
    case "success":
      return { icon: <Check size={11} />, className: "text-emerald-500" };
    case "error":
      return { icon: <CircleAlert size={11} />, className: "text-red-500" };
    case "denied":
      return { icon: <X size={11} />, className: "text-amber-500" };
    case "cancelled":
      return { icon: <X size={11} />, className: "text-neutral-400" };
    case "awaiting_approval":
      return { icon: <ShieldAlert size={11} />, className: "text-amber-500" };
    case "running":
      return { icon: <Loader2 size={11} className="animate-spin" />, className: "text-indigo-400" };
    default:
      return { icon: <Loader2 size={11} className="animate-spin" />, className: "text-neutral-400" };
  }
}

export function ToolRunCard({
  run,
  onOpenPath,
  onOpenDiff,
}: {
  run: ToolRunRecord;
  onOpenPath?: (path: string) => void;
  onOpenDiff?: (key: string, title: string, diff: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const meta = TOOL_META[run.tool_name] ?? { icon: Terminal, label: run.tool_name };
  const Icon = meta.icon;
  const visual = statusVisual(run.status);
  const pathArg = (run.args?.path as string) ?? null;
  const cmdArg = (run.args?.command as string) ?? null;

  return (
    <div className="my-1 overflow-hidden rounded-lg border border-neutral-200 dark:border-neutral-800">
      <button
        className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[11px] hover:bg-neutral-100 dark:hover:bg-neutral-800/50"
        onClick={() => setOpen(!open)}
      >
        <Icon size={12} className="shrink-0 text-neutral-500" />
        <span className="font-medium">{meta.label}</span>
        {pathArg && (
          <span
            className="cursor-pointer truncate font-mono text-indigo-400 hover:underline"
            onClick={(e) => {
              e.stopPropagation();
              onOpenPath?.(pathArg);
            }}
          >
            {pathArg}
          </span>
        )}
        {cmdArg && <span className="truncate font-mono text-neutral-500">{cmdArg}</span>}
        <span className="ml-auto flex items-center gap-2">
          {run.status === "success" && run.result_summary && (
            <span className="hidden max-w-48 truncate text-neutral-500 md:block">
              {run.result_summary}
            </span>
          )}
          <Badge
            tone={
              run.status === "success"
                ? "green"
                : run.status === "error"
                  ? "red"
                  : run.status === "denied"
                    ? "amber"
                    : run.status === "awaiting_approval"
                      ? "amber"
                      : "neutral"
            }
          >
            <span className={visual.className}>{visual.icon}</span>
            {run.status.replace("_", " ")}
          </Badge>
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </span>
      </button>
      {open && (
        <div className="border-t border-neutral-200 bg-neutral-50 px-3 py-2 text-[11px] dark:border-neutral-800 dark:bg-[#101218]">
          <div className="mb-1 font-semibold text-neutral-500">Arguments</div>
          <pre className="mb-2 max-h-40 overflow-auto rounded bg-neutral-200/60 p-2 font-mono text-[10px] dark:bg-neutral-800/60">
            {JSON.stringify(run.args, null, 2)}
          </pre>
          {run.result_summary && (
            <>
              <div className="mb-1 font-semibold text-neutral-500">Result</div>
              <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded bg-neutral-200/60 p-2 font-mono text-[10px] dark:bg-neutral-800/60">
                {run.result_summary}
              </pre>
            </>
          )}
          {run.error && (
            <>
              <div className="mb-1 font-semibold text-red-500">Error</div>
              <pre className="whitespace-pre-wrap rounded bg-red-500/10 p-2 font-mono text-[10px] text-red-400">
                {run.error}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}
