/** MCP servers: the user's global servers (~/.config/jenderalcode/mcp.json),
 *  the open project's `.mcp.json` (runs only once trusted), and the state and
 *  tools of the running servers. */
import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, Pencil, Plus, RotateCw, ShieldAlert, Trash2 } from "lucide-react";
import { api, type McpOverview, type McpServerConfig, type McpServerStatus } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { Badge, Button, IconButton, Spinner, Toggle, inputClass } from "@/components/ui";
import { confirmAction } from "@/lib/confirm";

const PRESETS: { label: string; config: McpServerConfig; hint: string }[] = [
  {
    label: "Playwright",
    hint: "Browser automation: open pages, click, fill forms, read the page.",
    config: blank({ name: "playwright", command: "npx", args: ["-y", "@playwright/mcp@latest"] }),
  },
  {
    label: "Laravel Boost",
    hint: "Laravel app context: routes, database schema and queries, logs, docs. Runs in the open project.",
    config: blank({ name: "laravel-boost", command: "php", args: ["artisan", "boost:mcp"] }),
  },
  {
    label: "Remote (HTTP)",
    hint: "A hosted MCP server, reached over streamable HTTP.",
    config: blank({ name: "", transport: "http", url: "https://" }),
  },
];

function blank(patch: Partial<McpServerConfig> = {}): McpServerConfig {
  return { name: "", transport: "stdio", command: "", args: [], env: {}, cwd: null, url: "", headers: {}, ...patch };
}

export function McpSettings() {
  const workspace = useApp((s) => s.workspace);
  const disabled = useApp((s) => s.settings.disabled_mcp_servers);
  const [overview, setOverview] = useState<McpOverview | null>(null);
  const [status, setStatus] = useState<McpServerStatus[]>([]);
  const [syncing, setSyncing] = useState(false);
  const [editing, setEditing] = useState<{ original: string | null; config: McpServerConfig } | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  const load = async () => {
    const o = await api.mcpOverview();
    setOverview(o);
    setStatus(o.status);
    return o;
  };

  /** Start/stop servers to match the config (only with a project open:
   *  servers run in the project directory). */
  const sync = async () => {
    if (!useApp.getState().workspace) return;
    setSyncing(true);
    try {
      setStatus(await api.mcpSync());
    } catch (e) {
      setMessage({ text: String(e), error: true });
    } finally {
      setSyncing(false);
    }
  };

  useEffect(() => {
    void load()
      .then(() => sync())
      .catch((e) => setMessage({ text: String(e), error: true }));
  }, [workspace?.id]);

  const act = async (fn: () => Promise<void>, done?: string) => {
    setMessage(null);
    try {
      await fn();
      await load();
      await sync();
      if (done) setMessage({ text: done, error: false });
    } catch (e) {
      setMessage({ text: String(e), error: true });
    }
  };

  const saveGlobal = (servers: McpServerConfig[], done: string) => act(() => api.mcpSaveGlobal(servers), done);

  const toggle = (name: string, enabled: boolean) =>
    act(() =>
      useApp.getState().updateSettings({
        disabled_mcp_servers: enabled ? disabled.filter((n) => n !== name) : [...disabled, name],
      }),
    );

  const trust = (trusted: boolean) =>
    act(async () => {
      await api.mcpTrustProject(trusted);
      // Trust is stored server-side: refresh the settings copy so a later
      // settings save does not drop it.
      useApp.setState({ settings: await api.getSettings() });
    }, trusted ? "Project servers trusted." : "Project servers stopped.");

  const restart = async (name: string) => {
    setSyncing(true);
    try {
      setStatus(await api.mcpRestart(name));
    } catch (e) {
      setMessage({ text: String(e), error: true });
    } finally {
      setSyncing(false);
    }
  };

  if (!overview) return <Spinner className="h-4 w-4" />;
  const global = overview.global;
  const project = overview.project;
  const statusOf = (name: string) => status.find((s) => s.name === name);

  return (
    <div className="space-y-4">
      {message && (
        <div
          className={`whitespace-pre-wrap rounded-[4px] px-2.5 py-1.5 text-[11px] ${
            message.error ? "bg-red-500/10 text-red-500" : "bg-accent-500/10 text-accent-700 dark:text-accent-300"
          }`}
        >
          {message.text}
        </div>
      )}

      {/* Global servers */}
      <div>
        <div className="mb-1.5 flex items-center gap-2">
          <span className="eyebrow">Your servers</span>
          {overview.global_path && (
            <span className="min-w-0 truncate font-mono text-[10px] text-neutral-500" title={overview.global_path}>
              {overview.global_path}
            </span>
          )}
          <span className="ml-auto flex items-center gap-1">
            {syncing && <Spinner className="h-3 w-3" />}
            <Button size="sm" disabled={syncing || !workspace} onClick={() => void sync()} title="Start or reconnect servers">
              <RotateCw size={11} /> Refresh
            </Button>
            <Button size="sm" variant="primary" onClick={() => setEditing({ original: null, config: blank() })}>
              <Plus size={11} /> Add server
            </Button>
          </span>
        </div>
        {overview.global_error && <div className="mb-1.5 text-[11px] text-red-500">{overview.global_error}</div>}
        {global.length === 0 && !editing && (
          <div className="rounded-[5px] border border-dashed border-neutral-300 px-3 py-3 text-[11px] text-neutral-500 dark:border-neutral-700">
            No MCP servers yet. Add one, e.g.{" "}
            {PRESETS.slice(0, 2).map((p, i) => (
              <span key={p.label}>
                {i > 0 && " or "}
                <button
                  className="text-accent-600 hover:underline dark:text-accent-400"
                  onClick={() => setEditing({ original: null, config: p.config })}
                >
                  {p.label}
                </button>
              </span>
            ))}
            .
          </div>
        )}
        <div className="space-y-1">
          {global.map((c) => (
            <ServerRow
              key={c.name}
              config={c}
              status={statusOf(c.name)}
              enabled={!disabled.includes(c.name)}
              shadowed={project?.trusted === true && project.servers.some((p) => p.name === c.name)}
              hasProject={!!workspace}
              onToggle={(on) => void toggle(c.name, on)}
              onRestart={() => void restart(c.name)}
              onEdit={() => setEditing({ original: c.name, config: c })}
              onDelete={async () => {
                if (await confirmAction(`Remove MCP server "${c.name}"?`)) {
                  void saveGlobal(global.filter((x) => x.name !== c.name), `Removed ${c.name}.`);
                }
              }}
            />
          ))}
        </div>
        {editing && (
          <ServerForm
            initial={editing.config}
            isNew={editing.original === null}
            onCancel={() => setEditing(null)}
            onSave={(config) => {
              const rest = global.filter((x) => x.name !== editing.original);
              if (rest.some((x) => x.name === config.name)) {
                setMessage({ text: `A server named "${config.name}" already exists.`, error: true });
                return;
              }
              const next = editing.original === null ? [...rest, config] : global.map((x) => (x.name === editing.original ? config : x));
              setEditing(null);
              void saveGlobal(next, `Saved ${config.name}.`);
            }}
          />
        )}
      </div>

      {/* Project servers */}
      {workspace && (
        <div>
          <div className="mb-1.5 flex items-center gap-2">
            <span className="eyebrow">This project</span>
            <span className="min-w-0 truncate font-mono text-[10px] text-neutral-500">{project?.path ?? ".mcp.json"}</span>
          </div>
          {!project ? (
            <div className="text-[11px] text-neutral-500">
              No <code className="font-mono">.mcp.json</code> in this project. Tools such as Laravel Boost (
              <code className="font-mono">php artisan boost:install</code>) create one.
            </div>
          ) : (
            <>
              {project.error && <div className="mb-1.5 text-[11px] text-red-500">{project.error}</div>}
              {!project.trusted && project.servers.length > 0 && (
                <div className="mb-1.5 flex items-start gap-2 rounded-[5px] border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-[11px]">
                  <ShieldAlert size={13} className="mt-0.5 shrink-0 text-amber-500" />
                  <div className="min-w-0 flex-1">
                    <div className="text-neutral-700 dark:text-neutral-300">
                      This project's <code className="font-mono">.mcp.json</code> wants to run these commands. A
                      repository can put any command here, so they run only after you trust this exact file
                      (editing it asks again).
                    </div>
                    <ul className="mt-1 space-y-0.5 font-mono text-[10.5px] text-neutral-600 dark:text-neutral-400">
                      {project.servers.map((s) => (
                        <li key={s.name}>
                          {s.name}: {s.transport === "stdio" ? [s.command, ...s.args].join(" ") : s.url}
                        </li>
                      ))}
                    </ul>
                    <Button size="sm" variant="primary" className="mt-2" onClick={() => void trust(true)}>
                      Trust and run
                    </Button>
                  </div>
                </div>
              )}
              {project.trusted && (
                <div className="space-y-1">
                  {project.servers.map((c) => (
                    <ServerRow
                      key={c.name}
                      config={c}
                      status={statusOf(c.name)}
                      enabled={!disabled.includes(c.name)}
                      hasProject
                      onToggle={(on) => void toggle(c.name, on)}
                      onRestart={() => void restart(c.name)}
                    />
                  ))}
                  <button
                    className="text-[11px] text-neutral-500 hover:text-red-500"
                    onClick={() => void trust(false)}
                  >
                    Stop trusting this file
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}

function ServerRow({
  config,
  status,
  enabled,
  shadowed,
  hasProject,
  onToggle,
  onRestart,
  onEdit,
  onDelete,
}: {
  config: McpServerConfig;
  status?: McpServerStatus;
  enabled: boolean;
  shadowed?: boolean;
  hasProject: boolean;
  onToggle: (on: boolean) => void;
  onRestart: () => void;
  onEdit?: () => void;
  onDelete?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const summary = config.transport === "stdio" ? [config.command, ...config.args].join(" ") : config.url;
  const state = !enabled ? "off" : shadowed ? "replaced" : !hasProject ? "idle" : (status?.state ?? "idle");
  const tone = state === "ready" ? "green" : state === "failed" ? "red" : state === "starting" ? "amber" : "neutral";
  const label =
    state === "ready"
      ? `${status!.tools.length} tool${status!.tools.length === 1 ? "" : "s"}`
      : state === "replaced"
        ? "replaced by project"
        : state === "idle"
          ? hasProject
            ? "not started"
            : "open a project"
          : state;
  return (
    <div className="rounded-[5px] border border-neutral-200 dark:border-neutral-800">
      <div className="flex items-center gap-2 px-2.5 py-1.5">
        <button className="flex min-w-0 flex-1 items-center gap-2 text-left" onClick={() => setOpen(!open)}>
          {open ? <ChevronDown size={11} className="shrink-0" /> : <ChevronRight size={11} className="shrink-0" />}
          <span className="shrink-0 text-xs font-medium">{config.name}</span>
          <span className="min-w-0 truncate font-mono text-[10px] text-neutral-500">{summary}</span>
        </button>
        <Badge tone={tone}>
          {state === "starting" && <Spinner className="h-2.5 w-2.5" />}
          {label}
        </Badge>
        {enabled && hasProject && (
          <IconButton title="Restart" onClick={onRestart}>
            <RotateCw size={11} />
          </IconButton>
        )}
        {onEdit && (
          <IconButton title="Edit" onClick={onEdit}>
            <Pencil size={11} />
          </IconButton>
        )}
        {onDelete && (
          <IconButton title="Remove" onClick={onDelete}>
            <Trash2 size={11} />
          </IconButton>
        )}
        <Toggle checked={enabled} onChange={onToggle} />
      </div>
      {open && (
        <div className="border-t border-neutral-200 px-3 py-2 text-[11px] dark:border-neutral-800">
          {status?.error && (
            <pre className="mb-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-[4px] bg-red-500/10 p-2 font-mono text-[10px] text-red-500">
              {status.error}
            </pre>
          )}
          {status?.tools.length ? (
            <ul className="space-y-1">
              {status.tools.map((t) => (
                <li key={t.name}>
                  <span className="font-mono text-[10.5px] text-accent-700 dark:text-accent-300">{t.tool}</span>
                  {t.description && (
                    <span className="ml-1.5 text-neutral-500">{t.description.split("\n")[0].slice(0, 160)}</span>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            !status?.error && <div className="text-neutral-500">No tools listed yet.</div>
          )}
        </div>
      )}
    </div>
  );
}

/** `KEY=value` / `Key: value` lines ⇄ a map. */
function toLines(map: Record<string, string>, sep: string) {
  return Object.entries(map)
    .map(([k, v]) => `${k}${sep}${v}`)
    .join("\n");
}
function fromLines(text: string, sep: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split("\n")) {
    const i = line.indexOf(sep);
    if (i > 0) out[line.slice(0, i).trim()] = line.slice(i + sep.length).trim();
  }
  return out;
}

function ServerForm({
  initial,
  isNew,
  onSave,
  onCancel,
}: {
  initial: McpServerConfig;
  isNew: boolean;
  onSave: (c: McpServerConfig) => void;
  onCancel: () => void;
}) {
  const [c, setC] = useState(initial);
  const [args, setArgs] = useState(initial.args.join("\n"));
  const [env, setEnv] = useState(toLines(initial.env, "="));
  const [headers, setHeaders] = useState(toLines(initial.headers, ": "));
  const [error, setError] = useState<string | null>(null);
  const stdio = c.transport === "stdio";

  const pick = (preset: McpServerConfig) => {
    setC(preset);
    setArgs(preset.args.join("\n"));
    setEnv(toLines(preset.env, "="));
    setHeaders(toLines(preset.headers, ": "));
  };

  const submit = () => {
    const config: McpServerConfig = {
      ...c,
      name: c.name.trim(),
      command: c.command.trim(),
      args: args.split("\n").map((a) => a.trim()).filter(Boolean),
      env: fromLines(env, "="),
      headers: fromLines(headers, ":"),
      url: c.url.trim(),
      cwd: c.cwd?.trim() || null,
    };
    if (!/^[A-Za-z0-9_.-]+$/.test(config.name)) return setError("Name: letters, digits, '-', '_' or '.' only.");
    if (stdio && !config.command) return setError("Enter the command that starts the server.");
    if (!stdio && !/^https?:\/\/.+/.test(config.url)) return setError("Enter the server's http(s) URL.");
    onSave(config);
  };

  const label = "mb-1 block text-[10.5px] font-medium text-neutral-500";
  return (
    <div className="animate-rise mt-2 rounded-[5px] border border-accent-500/50 p-3">
      {isNew && (
        <div className="mb-3 flex flex-wrap gap-1">
          {PRESETS.map((p) => (
            <button
              key={p.label}
              title={p.hint}
              onClick={() => pick(p.config)}
              className="rounded-[4px] border border-neutral-300 px-2 py-0.5 text-[11px] hover:border-accent-500 hover:text-accent-600 dark:border-neutral-700 dark:hover:text-accent-400"
            >
              {p.label}
            </button>
          ))}
        </div>
      )}
      <div className="grid grid-cols-[1fr_auto] gap-2">
        <label>
          <span className={label}>Name</span>
          <input className={inputClass} value={c.name} placeholder="playwright" onChange={(e) => setC({ ...c, name: e.target.value })} />
        </label>
        <label>
          <span className={label}>Type</span>
          <select
            className={inputClass}
            value={c.transport}
            onChange={(e) => setC({ ...c, transport: e.target.value as McpServerConfig["transport"] })}
          >
            <option value="stdio">Command (stdio)</option>
            <option value="http">HTTP</option>
          </select>
        </label>
      </div>
      {stdio ? (
        <>
          <label className="mt-2 block">
            <span className={label}>Command</span>
            <input className={`${inputClass} font-mono`} value={c.command} placeholder="npx" onChange={(e) => setC({ ...c, command: e.target.value })} />
          </label>
          <div className="mt-2 grid grid-cols-2 gap-2">
            <label>
              <span className={label}>Arguments (one per line)</span>
              <textarea className={`${inputClass} h-20 font-mono`} value={args} onChange={(e) => setArgs(e.target.value)} />
            </label>
            <label>
              <span className={label}>Environment (KEY=value per line)</span>
              <textarea className={`${inputClass} h-20 font-mono`} value={env} onChange={(e) => setEnv(e.target.value)} />
            </label>
          </div>
          <label className="mt-2 block">
            <span className={label}>Working directory (relative to the project; empty = project root)</span>
            <input className={`${inputClass} font-mono`} value={c.cwd ?? ""} onChange={(e) => setC({ ...c, cwd: e.target.value })} />
          </label>
        </>
      ) : (
        <>
          <label className="mt-2 block">
            <span className={label}>URL</span>
            <input className={`${inputClass} font-mono`} value={c.url} onChange={(e) => setC({ ...c, url: e.target.value })} />
          </label>
          <label className="mt-2 block">
            <span className={label}>Headers (Name: value per line, e.g. Authorization: Bearer …)</span>
            <textarea className={`${inputClass} h-16 font-mono`} value={headers} onChange={(e) => setHeaders(e.target.value)} />
          </label>
        </>
      )}
      <div className="mt-2 text-[10.5px] text-neutral-500">
        Stored in your home folder (readable only by you). Your shell's API keys are not passed to servers; put the
        ones a server needs under Environment.
      </div>
      {error && <div className="mt-2 text-[11px] text-red-500">{error}</div>}
      <div className="mt-3 flex justify-end gap-2">
        <Button size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" variant="primary" onClick={submit}>
          Save
        </Button>
      </div>
    </div>
  );
}
