/** Session history: search, resume, rename, delete. */
import { useEffect, useState } from "react";
import { Clock, MessageSquare, Pencil, Search, Trash2, GitBranch } from "lucide-react";
import { useChat } from "@/stores/chat";
import { useApp } from "@/stores/app";
import { api, type SessionSummary } from "@/lib/ipc";
import { Badge, Button, EmptyState, IconButton, inputClass } from "@/components/ui";
import { formatRelative, truncate } from "@/lib/format";

export function SessionsPage() {
  const sessions = useChat((s) => s.sessions);
  const { loadSessions, selectSession, removeSession, rename } = useChat.getState();
  const workspaces = useApp((s) => s.recent);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SessionSummary[] | null>(null);

  useEffect(() => {
    void loadSessions();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const runSearch = async (q: string) => {
    setQuery(q);
    if (!q.trim()) {
      setResults(null);
      return;
    }
    setResults(await api.searchSessions(q));
  };

  const list = results ?? sessions;
  const wsName = (id: string) => workspaces.find((w) => w.id === id)?.name ?? "Unknown project";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-3 border-b border-neutral-200 px-4 dark:border-neutral-800">
        <h1 className="flex items-center gap-2 text-sm font-semibold">
          <Clock size={15} className="text-indigo-400" /> Sessions
        </h1>
        <div className="relative ml-auto w-72">
          <Search size={12} className="absolute left-2.5 top-2.5 text-neutral-500" />
          <input
            className={`${inputClass} pl-7`}
            placeholder="Search conversations…"
            value={query}
            onChange={(e) => void runSearch(e.target.value)}
          />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {list.length === 0 && (
          <EmptyState
            icon={<MessageSquare size={28} />}
            title={query ? "No matching sessions" : "No sessions yet"}
            hint={query ? undefined : "Start a conversation in the workspace."}
          />
        )}
        <div className="overflow-hidden rounded-xl border border-neutral-200 dark:border-neutral-800">
          {list.map((s, i) => (
            <div
              key={s.id}
              className={`group flex items-center gap-3 px-4 py-3 hover:bg-neutral-100 dark:hover:bg-neutral-800/60 ${
                i > 0 ? "border-t border-neutral-200 dark:border-neutral-800" : ""
              }`}
            >
              <button
                className="flex min-w-0 flex-1 flex-col items-start gap-0.5 text-left"
                onClick={async () => {
                  const ws = workspaces.find((w) => w.id === s.workspace_id);
                  if (ws) {
                    await useApp.getState().openWorkspacePath(ws.root_path);
                    await selectSession(s.id);
                    useApp.getState().setView("workspace");
                  }
                }}
              >
                <div className="flex items-center gap-2 text-xs font-medium">
                  {truncate(s.title || "Untitled", 64)}
                  {s.message_count > 0 && (
                    <Badge tone="neutral">{s.message_count} msg</Badge>
                  )}
                  {s.policy_mode === "yolo" && <Badge tone="amber">full access</Badge>}
                </div>
                <div className="flex items-center gap-2 text-[11px] text-neutral-500">
                  <GitBranch size={9} /> {wsName(s.workspace_id)}
                  {s.model_name && <span>· {s.model_name}</span>}
                  <span>· {formatRelative(s.updated_at)}</span>
                </div>
              </button>
              <IconButton
                title="Rename"
                onClick={async () => {
                  const title = window.prompt("New title", s.title);
                  if (title) await rename(s.id, title);
                }}
              >
                <Pencil size={12} />
              </IconButton>
              <IconButton
                title="Delete"
                onClick={async () => {
                  if (window.confirm(`Delete session "${s.title}" and its history?`))
                    await removeSession(s.id);
                }}
              >
                <Trash2 size={12} />
              </IconButton>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
