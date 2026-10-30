/** Welcome / Projects: open a folder, pick a working layout, resume a
 *  recent project. */
import { useState } from "react";
import { ArrowUpRight, FolderOpen, GitBranch, Plug, Trash2 } from "lucide-react";
import { useApp, type LayoutMode } from "@/stores/app";
import { Button } from "@/components/ui";
import { Logo, Wordmark } from "@/components/Logo";
import { formatRelative } from "@/lib/format";

const MODES: { id: LayoutMode; title: string; desc: string }[] = [
  {
    id: "chat",
    title: "Chat",
    desc: "Conversation first. Changes, files, browser, terminal and git sit in a side panel.",
  },
  {
    id: "code",
    title: "Code",
    desc: "Full IDE: explorer, editor and terminal, with the agent beside your code.",
  },
];

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
    <div className="h-full overflow-y-auto">
      <div className="mx-auto grid min-h-full max-w-6xl grid-cols-1 gap-x-14 gap-y-10 px-10 py-12 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        {/* Left: identity, actions, layout */}
        <section className="stagger flex flex-col">
          <div style={{ "--i": 0 } as React.CSSProperties} className="mb-10 flex items-center gap-3">
            <Logo size={34} />
            <Wordmark className="text-lg" />
          </div>

          <div style={{ "--i": 1 } as React.CSSProperties} className="eyebrow mb-3">
            Local-first · any model · human-approved
          </div>
          <h1
            style={{ "--i": 2 } as React.CSSProperties}
            className="font-display text-[44px] leading-[0.98] font-semibold text-neutral-900 dark:text-neutral-50"
          >
            Command
            <br />
            your codebase<span className="text-accent-500">.</span>
          </h1>
          <p style={{ "--i": 3 } as React.CSSProperties} className="mt-4 max-w-sm text-[13px] leading-relaxed text-neutral-500">
            Point an agent at a folder on this machine. It reads, edits, runs and checks — and stops to ask before
            anything you haven't allowed.
          </p>

          <div style={{ "--i": 4 } as React.CSSProperties} className="mt-7 flex flex-wrap gap-2">
            <Button variant="primary" onClick={guard(openFolderPicker)} className="px-3.5 py-2">
              <FolderOpen size={14} /> Open project folder
            </Button>
            <Button onClick={() => useApp.getState().setView("providers")} className="px-3.5 py-2">
              <Plug size={14} /> Providers
            </Button>
          </div>

          {error && (
            <div className="mt-4 border-l-2 border-red-500 bg-red-500/5 px-3 py-2 text-xs text-red-500">{error}</div>
          )}

          <div style={{ "--i": 5 } as React.CSSProperties} className="mt-12">
            <div className="eyebrow mb-3">Working layout</div>
            <div className="grid grid-cols-2 gap-3">
              {MODES.map((m) => {
                const active = layout === m.id;
                return (
                  <button
                    key={m.id}
                    onClick={() => setLayout(m.id)}
                    aria-pressed={active}
                    className={`group rounded-md border p-3 text-left transition-colors ${
                      active
                        ? "ticks border-accent-500/60 bg-white dark:bg-neutral-900"
                        : "border-neutral-200 hover:border-neutral-400 dark:border-neutral-800 dark:hover:border-neutral-600"
                    }`}
                  >
                    <LayoutSchematic mode={m.id} active={active} />
                    <div className="mt-3 flex items-center gap-2">
                      <span className="font-display text-[15px] font-semibold">{m.title}</span>
                      {active && <span className="h-1.5 w-1.5 rounded-full bg-accent-500" />}
                    </div>
                    <div className="mt-1 text-[11px] leading-snug text-neutral-500">{m.desc}</div>
                  </button>
                );
              })}
            </div>
          </div>
        </section>

        {/* Right: recent projects manifest */}
        <section className="flex flex-col lg:pt-[92px]">
          <div className="mb-3 flex items-baseline justify-between">
            <div className="eyebrow">Recent projects</div>
            <div className="font-mono text-[10px] text-neutral-500">{String(recent.length).padStart(2, "0")}</div>
          </div>

          {recent.length === 0 ? (
            <div className="dot-grid flex flex-1 flex-col items-start justify-end rounded-md border border-dashed border-neutral-300 p-6 dark:border-neutral-800">
              <div className="font-display text-lg font-semibold">Nothing here yet.</div>
              <div className="mt-1 max-w-xs text-xs text-neutral-500">
                Open a folder to start. Your code stays on this machine; only what you send goes to the model provider.
              </div>
            </div>
          ) : (
            <ol className="stagger border-t border-neutral-200 dark:border-neutral-800">
              {recent.map((w, i) => (
                <li
                  key={w.id}
                  style={{ "--i": i } as React.CSSProperties}
                  className="group relative border-b border-neutral-200 dark:border-neutral-800"
                >
                  <span className="absolute inset-y-0 left-0 w-[3px] origin-top scale-y-0 bg-accent-500 transition-transform duration-200 group-hover:scale-y-100" />
                  <button
                    className="grid w-full grid-cols-[2.25rem_minmax(0,1fr)_auto] items-center gap-3 py-3.5 pr-10 pl-3 text-left"
                    onClick={guard(() => openWorkspacePath(w.root_path))}
                  >
                    <span className="font-mono text-[11px] text-neutral-400 group-hover:text-accent-600 dark:group-hover:text-accent-400">
                      {String(i + 1).padStart(2, "0")}
                    </span>
                    <span className="min-w-0">
                      <span className="flex items-center gap-2">
                        <span className="truncate font-display text-[16px] font-semibold text-neutral-900 dark:text-neutral-100">
                          {w.name}
                        </span>
                        {w.is_git_repo && (
                          <span className="inline-flex shrink-0 items-center gap-0.5 font-mono text-[10px] text-neutral-500">
                            <GitBranch size={10} /> git
                          </span>
                        )}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-neutral-500">{w.root_path}</span>
                    </span>
                    <span className="flex items-center gap-2 font-mono text-[10px] text-neutral-500">
                      {formatRelative(w.last_opened_at)}
                      <ArrowUpRight
                        size={13}
                        className="text-neutral-400 transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5 group-hover:text-accent-500"
                      />
                    </span>
                  </button>
                  <button
                    title="Remove from list"
                    onClick={guard(() => removeRecent(w.id))}
                    className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-1 text-neutral-400 opacity-0 transition-opacity group-hover:opacity-100 hover:text-red-500"
                  >
                    <Trash2 size={12} />
                  </button>
                </li>
              ))}
            </ol>
          )}

          <div className="mt-auto pt-10 font-mono text-[10.5px] leading-relaxed text-neutral-500">
            <Kbd>⌘K</Kbd> command palette &nbsp;·&nbsp; <Kbd>⌘+</Kbd>/<Kbd>⌘−</Kbd> zoom &nbsp;·&nbsp; the built-in{" "}
            <span className="text-neutral-700 dark:text-neutral-300">Mock</span> provider runs the full agent loop without
            an API key.
          </div>
        </section>
      </div>
    </div>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="rounded-[3px] border border-neutral-300 bg-white px-1 py-px text-[10px] text-neutral-700 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-300">
      {children}
    </kbd>
  );
}

/** Tiny wireframe of each layout, drawn with divs. */
function LayoutSchematic({ mode, active }: { mode: LayoutMode; active: boolean }) {
  const line = active ? "bg-accent-500/70" : "bg-neutral-300 dark:bg-neutral-700";
  const pane = "border-neutral-300 dark:border-neutral-700";
  if (mode === "chat") {
    return (
      <div className={`flex h-16 overflow-hidden rounded-[3px] border ${pane}`}>
        <div className={`w-[18%] space-y-1 border-r p-1 ${pane}`}>
          <div className={`h-1 w-full ${line}`} />
          <div className="h-1 w-3/4 bg-neutral-300/70 dark:bg-neutral-700/70" />
          <div className="h-1 w-2/3 bg-neutral-300/70 dark:bg-neutral-700/70" />
        </div>
        <div className="flex flex-1 flex-col justify-end gap-1 p-1.5">
          <div className="ml-auto h-1.5 w-1/2 rounded-sm bg-neutral-300 dark:bg-neutral-700" />
          <div className="h-1 w-5/6 bg-neutral-300/70 dark:bg-neutral-700/70" />
          <div className="h-1 w-2/3 bg-neutral-300/70 dark:bg-neutral-700/70" />
          <div className={`mt-0.5 h-2.5 rounded-sm border ${active ? "border-accent-500/70" : pane}`} />
        </div>
        <div className={`w-[26%] space-y-1 border-l p-1 ${pane}`}>
          <div className={`h-1 w-2/3 ${line}`} />
          <div className="h-1 w-full bg-neutral-300/70 dark:bg-neutral-700/70" />
          <div className="h-1 w-5/6 bg-neutral-300/70 dark:bg-neutral-700/70" />
        </div>
      </div>
    );
  }
  return (
    <div className={`flex h-16 overflow-hidden rounded-[3px] border ${pane}`}>
      <div className={`w-[18%] space-y-1 border-r p-1 ${pane}`}>
        <div className="h-1 w-full bg-neutral-300/70 dark:bg-neutral-700/70" />
        <div className="h-1 w-3/4 bg-neutral-300/70 dark:bg-neutral-700/70" />
        <div className="h-1 w-5/6 bg-neutral-300/70 dark:bg-neutral-700/70" />
      </div>
      <div className="flex flex-1 flex-col">
        <div className="flex-1 space-y-1 p-1.5">
          <div className={`h-1 w-1/2 ${line}`} />
          <div className="h-1 w-3/4 bg-neutral-300/70 dark:bg-neutral-700/70" />
          <div className="h-1 w-2/3 bg-neutral-300/70 dark:bg-neutral-700/70" />
        </div>
        <div className={`h-3.5 border-t ${pane} bg-neutral-200/60 dark:bg-neutral-800/60`} />
      </div>
      <div className={`w-[28%] space-y-1 border-l p-1 ${pane}`}>
        <div className="ml-auto h-1.5 w-2/3 rounded-sm bg-neutral-300 dark:bg-neutral-700" />
        <div className="h-1 w-full bg-neutral-300/70 dark:bg-neutral-700/70" />
        <div className={`mt-1 h-2 rounded-sm border ${active ? "border-accent-500/70" : pane}`} />
      </div>
    </div>
  );
}
