/** Tool-run card: shows agent tool activity with status and results. */
import { useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  FileEdit,
  FilePlus,
  FileSearch,
  FolderTree,
  BookOpen,
  Bot,
  Globe,
  ListChecks,
  Plug,
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
  fs_multi_edit: { icon: FileEdit, label: "Edit file" },
  web_fetch: { icon: Globe, label: "Fetch" },
  todo_write: { icon: ListChecks, label: "Todos" },
  task: { icon: Bot, label: "Sub-agent" },
  skill: { icon: BookOpen, label: "Skill" },
  fs_delete: { icon: Trash2, label: "Delete" },
  fs_list: { icon: FolderTree, label: "List directory" },
  fs_search: { icon: Search, label: "Search" },
  glob_files: { icon: Search, label: "Glob files" },
  terminal_run: { icon: Terminal, label: "Run command" },
  git_status: { icon: GitBranch, label: "Git status" },
  git_diff: { icon: GitBranch, label: "Git diff" },
  git_log: { icon: GitBranch, label: "Git log" },
};

/** `mcp__playwright__browser_click` → "playwright · browser_click". */
function mcpMeta(name: string): { icon: typeof FileEdit; label: string } | null {
  const m = /^mcp__(.+?)__(.+)$/.exec(name);
  return m ? { icon: Plug, label: `${m[1]} · ${m[2]}` } : null;
}

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
      return { icon: <Loader2 size={11} className="animate-spin" />, className: "text-accent-600 dark:text-accent-400" };
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
  const meta = TOOL_META[run.tool_name] ?? mcpMeta(run.tool_name) ?? { icon: Terminal, label: run.tool_name };
  const Icon = meta.icon;
  const visual = statusVisual(run.status);
  const pathArg = (run.args?.path as string) ?? null;
  const cmdArg = ((run.args?.command ?? run.args?.url ?? run.args?.description ?? run.args?.name) as string) ?? null;
  const todos = run.tool_name === "todo_write" && run.status === "success" ? parseTodos(run.args?.todos) : null;

  return (
    <div className="my-1 overflow-hidden rounded-md border border-neutral-200 dark:border-neutral-800">
      <button
        className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-[11px] hover:bg-neutral-100 dark:hover:bg-neutral-800/50"
        onClick={() => setOpen(!open)}
      >
        <Icon size={12} className="shrink-0 text-neutral-500" />
        <span className="font-medium">{meta.label}</span>
        {pathArg && (
          <span
            className="cursor-pointer truncate font-mono text-accent-600 dark:text-accent-400 hover:underline"
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
      {todos && <TodoList todos={todos} />}
      {open && (
        <div className="border-t border-neutral-200 bg-neutral-50 px-3 py-2 text-[11px] dark:border-neutral-800 dark:bg-neutral-925">
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

interface Todo {
  content: string;
  status: "pending" | "in_progress" | "completed";
}

/** The newest successful `todo_write` list of the session. */
export function latestTodos(runs: ToolRunRecord[]): { runId: string; todos: Todo[] } | null {
  let best: ToolRunRecord | null = null;
  for (const r of runs) {
    if (r.tool_name === "todo_write" && r.status === "success" && (!best || r.created_at >= best.created_at)) best = r;
  }
  const todos = best ? parseTodos(best.args?.todos) : null;
  return best && todos ? { runId: best.id, todos } : null;
}

/** The agent's current checklist, pinned above the composer. Collapsed it
 *  shows the step in progress; a finished list can be dismissed until the
 *  agent writes a new one. */
export function TodoDock({ runs }: { runs: ToolRunRecord[] }) {
  const latest = useMemo(() => latestTodos(runs), [runs]);
  const [collapsed, setCollapsed] = useState(false);
  const [dismissed, setDismissed] = useState<string | null>(null);
  if (!latest || latest.runId === dismissed) return null;
  const { todos } = latest;
  const done = todos.filter((t) => t.status === "completed").length;
  const allDone = done === todos.length;
  const current = todos.find((t) => t.status === "in_progress") ?? todos.find((t) => t.status === "pending");
  return (
    <div className="animate-rise mb-1.5 overflow-hidden rounded-[5px] border border-neutral-200 bg-white shadow-[0_10px_28px_-22px_rgba(0,0,0,0.6)] dark:border-neutral-800 dark:bg-neutral-900">
      <div className="flex items-center gap-2 px-2.5 py-1">
        <button
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          title={collapsed ? "Show the agent's todo list" : "Collapse"}
          onClick={() => setCollapsed(!collapsed)}
        >
          {collapsed ? <ChevronRight size={11} className="shrink-0" /> : <ChevronDown size={11} className="shrink-0" />}
          <ListChecks size={12} className="shrink-0 text-accent-600 dark:text-accent-400" />
          <span className="eyebrow shrink-0">Todos</span>
          <span className="shrink-0 font-mono text-[10px] text-neutral-500">
            {done}/{todos.length}
          </span>
          <span className="h-1 w-16 shrink-0 overflow-hidden rounded-[1px] bg-neutral-200 dark:bg-neutral-800">
            <span className="block h-full bg-accent-500 transition-[width]" style={{ width: `${(done / todos.length) * 100}%` }} />
          </span>
          {collapsed && (
            <span className="min-w-0 truncate text-[11.5px] text-neutral-600 dark:text-neutral-400">
              {allDone ? "All done" : current?.content}
            </span>
          )}
        </button>
        {allDone && (
          <button
            title="Dismiss"
            onClick={() => setDismissed(latest.runId)}
            className="shrink-0 rounded-[3px] p-0.5 text-neutral-500 hover:bg-neutral-200 hover:text-neutral-900 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
          >
            <X size={11} />
          </button>
        )}
      </div>
      {!collapsed && (
        <div className="max-h-44 overflow-y-auto">
          <TodoList todos={todos} />
        </div>
      )}
    </div>
  );
}

function parseTodos(value: unknown): Todo[] | null {
  if (!Array.isArray(value)) return null;
  const todos = value.filter(
    (t): t is Todo => typeof t?.content === "string" && ["pending", "in_progress", "completed"].includes(t?.status),
  );
  return todos.length ? todos : null;
}

/** The agent's checklist from a `todo_write` call. */
function TodoList({ todos }: { todos: Todo[] }) {
  return (
    <ul className="border-t border-neutral-200 px-3 py-1.5 dark:border-neutral-800">
      {todos.map((t, i) => (
        <li key={i} className="flex items-start gap-2 py-0.5 text-[11.5px]">
          <span
            className={`mt-[3px] flex h-3 w-3 shrink-0 items-center justify-center rounded-[3px] border ${
              t.status === "completed"
                ? "border-accent-500 bg-accent-500 text-white dark:text-neutral-950"
                : t.status === "in_progress"
                  ? "border-accent-500"
                  : "border-neutral-400 dark:border-neutral-600"
            }`}
          >
            {t.status === "completed" && <Check size={9} strokeWidth={3} />}
            {t.status === "in_progress" && <span className="h-1.5 w-1.5 animate-pulse rounded-[1px] bg-accent-500" />}
          </span>
          <span
            className={
              t.status === "completed"
                ? "text-neutral-500 line-through"
                : t.status === "in_progress"
                  ? "font-medium text-neutral-900 dark:text-neutral-100"
                  : "text-neutral-700 dark:text-neutral-300"
            }
          >
            {t.content}
          </span>
        </li>
      ))}
    </ul>
  );
}
