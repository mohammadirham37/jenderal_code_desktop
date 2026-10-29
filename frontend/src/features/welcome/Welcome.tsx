/** Welcome / Projects page: recent workspaces, open folder, quick links. */
import { useState } from "react";
import { FolderOpen, FolderPlus, Clock, Trash2, GitBranch, Sparkles, MessagesSquare, Code2 } from "lucide-react";
import { useApp, type LayoutMode } from "@/stores/app";
import { Button, EmptyState } from "@/components/ui";
import { formatRelative } from "@/lib/format";

export function Welcome() {
  const recent = useApp((s) => s.recent);
  const layout = useApp((s) => s.layout);
  const { openWorkspacePath, openFolderPicker, removeRecent, setLayout } = useApp.getState();
  const [error, setError] = useState<string | null>(null);

  const guard = (fn: () => Promise<void>) => async () => {
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="flex h-full items-start justify-center overflow-y-auto p-10">
      <div className="w-full max-w-2xl">
        <div className="mb-8 flex items-center gap-4">
          <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-gradient-to-br from-indigo-500 to-violet-600 text-2xl font-bold text-white shadow-lg">
            J
          </div>
          <div>
            <h1 className="text-xl font-semibold">JenderalCode Desktop</h1>
            <p className="text-xs text-neutral-500">
              One workspace, many agents — local-first AI coding
            </p>
          </div>
        </div>

        <div className="mb-6 flex gap-2">
          <Button variant="primary" onClick={guard(openFolderPicker)}>
            <FolderOpen size={14} /> Open project folder
          </Button>
          <Button onClick={() => useApp.getState().setView("providers")}>
            <Sparkles size={14} /> Configure providers
          </Button>
        </div>

        {error && (
          <div className="mb-4 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-400">{error}</div>
        )}

        <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-neutral-500">How do you want to work?</h2>
        <div className="mb-6 grid grid-cols-2 gap-2">
          {(
            [
              {
                id: "chat",
                icon: MessagesSquare,
                title: "Chat",
                desc: "Talk to the agent in a wide conversation view. Changes, files, terminal, and git live in a side panel.",
              },
              {
                id: "code",
                icon: Code2,
                title: "Code",
                desc: "Full IDE layout: file explorer, editor, terminal, and the agent chat beside your code.",
              },
            ] as { id: LayoutMode; icon: typeof Code2; title: string; desc: string }[]
          ).map(({ id, icon: Icon, title, desc }) => (
            <button
              key={id}
              onClick={() => setLayout(id)}
              className={`rounded-xl border p-3 text-left transition-colors ${
                layout === id
                  ? "border-indigo-500/70 bg-indigo-500/5 ring-1 ring-indigo-500/30"
                  : "border-neutral-200 hover:border-neutral-300 dark:border-neutral-800 dark:hover:border-neutral-700"
              }`}
            >
              <div className="mb-1 flex items-center gap-1.5 text-sm font-medium">
                <Icon size={14} className={layout === id ? "text-indigo-500" : "text-neutral-500"} /> {title}
                {layout === id && <span className="ml-auto text-[9px] uppercase text-indigo-500">selected</span>}
              </div>
              <div className="text-[11px] leading-snug text-neutral-500">{desc}</div>
            </button>
          ))}
        </div>

        <h2 className="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
          <Clock size={13} /> Recent projects
        </h2>

        {recent.length === 0 ? (
          <div className="rounded-xl border border-dashed border-neutral-300 dark:border-neutral-700">
            <EmptyState
              icon={<FolderPlus size={28} />}
              title="No projects yet"
              hint="Open a local folder to start coding with AI. Your project stays on your machine."
            />
          </div>
        ) : (
          <div className="overflow-hidden rounded-xl border border-neutral-200 dark:border-neutral-800">
            {recent.map((w, i) => (
              <div
                key={w.id}
                className={`group flex items-center gap-3 px-4 py-3 hover:bg-neutral-100 dark:hover:bg-neutral-800/60 ${
                  i > 0 ? "border-t border-neutral-200 dark:border-neutral-800" : ""
                }`}
              >
                <button
                  className="flex min-w-0 flex-1 items-center gap-3 text-left"
                  onClick={guard(() => openWorkspacePath(w.root_path))}
                >
                  <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-indigo-500/10 text-indigo-500">
                    <FolderOpen size={15} />
                  </div>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2 text-sm font-medium">
                      {w.name}
                      {w.is_git_repo && (
                        <span className="inline-flex items-center gap-0.5 text-[10px] text-neutral-500">
                          <GitBranch size={10} /> git
                        </span>
                      )}
                    </div>
                    <div className="truncate text-[11px] text-neutral-500">{w.root_path}</div>
                  </div>
                </button>
                <span className="hidden text-[11px] text-neutral-500 sm:block">
                  {formatRelative(w.last_opened_at)}
                </span>
                <button
                  title="Remove from list"
                  onClick={async () => await removeRecent(w.id)}
                  className="rounded p-1 text-neutral-400 opacity-0 transition-opacity group-hover:opacity-100 hover:bg-neutral-200 hover:text-red-500 dark:hover:bg-neutral-700"
                >
                  <Trash2 size={13} />
                </button>
              </div>
            ))}
          </div>
        )}

        <p className="mt-8 text-[11px] leading-relaxed text-neutral-500">
          Tip: press <kbd className="rounded border border-neutral-300 px-1 dark:border-neutral-700">⌘K</kbd>{" "}
          for the command palette — switch between Chat and Code anytime from the project header. The built-in <b>Mock provider</b> lets you try the full agent
          loop — including tool permissions and approvals — without any API key.
        </p>
      </div>
    </div>
  );
}
