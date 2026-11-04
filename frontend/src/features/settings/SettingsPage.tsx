/** Settings: appearance, permissions, usage dashboard, privacy & data. */
import { useEffect, useState } from "react";
import {
  BarChart3,
  Bot,
  Coins,
  Database,
  Download,
  Eye,
  EyeOff,
  Keyboard,
  Palette,
  ShieldCheck,
  BookOpen,
  Trash2, Plug, ArrowUpCircle, Smartphone } from "lucide-react";
import { api, type UsageSummary } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { Button, Field, inputClass } from "@/components/ui";
import { formatCost, formatTokens } from "@/lib/format";
import { AgentCliSettings } from "./AgentCliSettings";
import { SkillsSettings } from "./SkillsSettings";
import { McpSettings } from "./McpSettings";
import { UpdateSettings } from "./UpdateSettings";
import { RemoteSettings } from "./RemoteSettings";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { confirmAction } from "@/lib/confirm";

const SHORTCUTS: [string, string][] = [
  ["⌘K / Ctrl+K", "Command palette"],
  ["⌘S / Ctrl+S", "Save active file"],
  ["⌘+ / ⌘− / ⌘0 (Ctrl+ / Ctrl− / Ctrl+0)", "Zoom in / out / reset"],
  ["Enter", "Send message (chat input)"],
  ["Shift+Enter", "Newline in chat input"],
];

export function SettingsPage() {
  const { settings, updateSettings } = useApp();
  const zoom = useApp((s) => s.zoom);
  const zoomIn = useApp((s) => s.zoomIn);
  const zoomOut = useApp((s) => s.zoomOut);
  const resetZoom = useApp((s) => s.resetZoom);
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [dataDir, setDataDir] = useState<string>("");
  const [credBackend, setCredBackend] = useState<string>("");

  const switchCredStorage = async (target: "file" | "keychain") => {
    const label = target === "keychain" ? "OS keychain" : "local file (0600)";
    if (
      !await confirmAction(
        `Move stored API keys to the ${label}?\n\nExisting keys are migrated automatically. ` +
          (target === "keychain"
            ? "Note: macOS may ask for your login password once when moving keys into the keychain (unsigned builds)."
            : "The local file never triggers password prompts."),
      )
    ) {
      return;
    }
    try {
      const n = await api.switchCredentialStorage(target);
      setMessage(
        n > 0
          ? `Moved ${n} API key(s) to the ${label}.`
          : `Credential storage set to ${label}.`,
      );
      setCredBackend(await api.getCredentialBackend());
    } catch (e) {
      setMessage(`Switch failed: ${e}`);
    }
  };

  const refreshUsage = async () => setUsage(await api.usageSummary());
  useEffect(() => {
    void refreshUsage();
    void api.getDataDir().then(setDataDir).catch(() => {});
    void api.getCredentialBackend().then(setCredBackend).catch(() => {});
  }, []);

  const maxDay = Math.max(1, ...(usage?.by_day.map((d) => d.input_tokens + d.output_tokens) ?? [1]));

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-14 shrink-0 items-center border-b border-neutral-200 px-4 dark:border-neutral-800">
        <h1 className="flex items-center gap-2.5 font-display text-[19px] font-semibold tracking-tight">
          <Palette size={16} className="text-accent-500" /> Usage &amp; Settings
        </h1>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        <div className="mx-auto max-w-3xl space-y-8">
          {/* Updates */}
          <section id="updates">
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <ArrowUpCircle size={13} /> Updates
            </h2>
            <UpdateSettings />
          </section>

          {/* Remote control */}
          <section id="remote">
            <h2 className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Smartphone size={13} /> Remote control
            </h2>
            <p className="mb-3 text-[11px] text-neutral-500">
              Chat with your sessions and answer approvals from your phone, through a relay server. Messages are
              end-to-end encrypted between this desktop and the phones you pair here; the relay only forwards them.
              Approvals from the phone cover one action at a time, and sessions with full access stay desktop-only.
            </p>
            <RemoteSettings />
          </section>

          {/* Usage */}
          <section>
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <BarChart3 size={13} /> Usage
            </h2>
            {usage ? (
              <>
                <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-4">
                  <StatCard label="Input tokens" value={formatTokens(usage.total_input)} />
                  <StatCard label="Output tokens" value={formatTokens(usage.total_output)} />
                  <StatCard label="Est. cost" value={formatCost(usage.total_cost)} />
                  <StatCard label="Sessions" value={String(usage.sessions)} />
                </div>
                {usage.by_day.length > 0 && (
                  <div className="mb-4 rounded-md border border-neutral-200 p-4 dark:border-neutral-800">
                    <div className="mb-2 text-[11px] font-medium text-neutral-500">
                      Tokens per day (last 30 days)
                    </div>
                    <div className="flex h-28 items-end gap-1">
                      {usage.by_day.map((d) => {
                        const total = d.input_tokens + d.output_tokens;
                        return (
                          <div
                            key={d.day}
                            title={`${d.day}: ${formatTokens(total)} in+out${d.cost_estimate ? ` · ${formatCost(d.cost_estimate)}` : ""}`}
                            className="min-w-2 flex-1 bg-accent-500/75 transition-colors hover:bg-accent-400"
                            style={{ height: `${(total / maxDay) * 100}%` }}
                          />
                        );
                      })}
                    </div>
                  </div>
                )}
                {usage.by_provider.length > 0 && (
                  <div className="rounded-md border border-neutral-200 p-4 dark:border-neutral-800">
                    <div className="mb-2 text-[11px] font-medium text-neutral-500">By provider</div>
                    {usage.by_provider.map((p) => (
                      <div key={p.provider_id} className="flex items-center justify-between py-1 text-xs">
                        <span className="font-mono text-neutral-500">{p.provider_id}</span>
                        <span className="flex gap-4 text-neutral-500">
                          <span>↑{formatTokens(p.input_tokens)}</span>
                          <span>↓{formatTokens(p.output_tokens)}</span>
                          <span className="w-16 text-right">{formatCost(p.cost_estimate)}</span>
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </>
            ) : (
              <div className="text-xs text-neutral-500">Loading usage…</div>
            )}
          </section>

          {/* Appearance */}
          <section>
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Eye size={13} /> Appearance
            </h2>
            <div className="grid gap-4 md:grid-cols-2">
              <Field label="Theme">
                <select
                  className={inputClass}
                  value={settings.theme}
                  onChange={(e) => void updateSettings({ theme: e.target.value })}
                >
                  <option value="dark">Dark</option>
                  <option value="light">Light</option>
                  <option value="system">System</option>
                </select>
              </Field>
              <Field label="Show hidden files in explorer">
                <select
                  className={inputClass}
                  value={settings.show_hidden_files ? "yes" : "no"}
                  onChange={(e) => void updateSettings({ show_hidden_files: e.target.value === "yes" })}
                >
                  <option value="no">No (recommended)</option>
                  <option value="yes">Yes</option>
                </select>
              </Field>
              <Field
                label="Editor word wrap"
                hint="Wrap long lines down to the next row instead of scrolling left/right. Applies immediately."
              >
                <select
                  className={inputClass}
                  value={settings.editor_word_wrap ? "wrap" : "nowrap"}
                  onChange={(e) => void updateSettings({ editor_word_wrap: e.target.value === "wrap" })}
                >
                  <option value="wrap">Wrap long lines (recommended)</option>
                  <option value="nowrap">No wrap — horizontal scroll</option>
                </select>
              </Field>
              <Field label="Editor font size">
                <input
                  type="number"
                  min={10}
                  max={20}
                  className={inputClass}
                  value={settings.editor_font_size}
                  onChange={(e) => void updateSettings({ editor_font_size: Number(e.target.value) })}
                />
              </Field>
              <Field label="Terminal font size">
                <input
                  type="number"
                  min={10}
                  max={20}
                  className={inputClass}
                  value={settings.terminal_font_size}
                  onChange={(e) => void updateSettings({ terminal_font_size: Number(e.target.value) })}
                />
              </Field>
            </div>
            <Field
              label="Zoom"
              hint={`Keyboard: ${navigator.platform.toUpperCase().includes("MAC") ? "⌘+ / ⌘−" : "Ctrl+ / Ctrl−"} to zoom, ${navigator.platform.toUpperCase().includes("MAC") ? "⌘0" : "Ctrl+0"} to reset. The chosen zoom is remembered.`}
            >
              <div className="flex items-center gap-2">
                <Button size="sm" onClick={() => void zoomOut()} disabled={zoom <= 0.7}>
                  −
                </Button>
                <span className="w-14 text-center font-mono text-xs">
                  {Math.round(zoom * 100)}%
                </span>
                <Button size="sm" onClick={() => void zoomIn()} disabled={zoom >= 1.6}>
                  +
                </Button>
                <Button size="sm" onClick={() => void resetZoom()} disabled={zoom === 1}>
                  Reset
                </Button>
              </div>
            </Field>
          </section>

          {/* Skills */}
          <section id="skills">
            <h2 className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <BookOpen size={13} /> Skills
            </h2>
            <p className="mb-3 text-[11px] text-neutral-500">
              Packaged instructions the agent loads when a task matches (brainstorming, planning, debugging, …). Type
              <code className="mx-1 font-mono">/</code>in the chat to run one directly.
            </p>
            <SkillsSettings />
          </section>

          {/* MCP servers */}
          <section id="mcp">
            <h2 className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Plug size={13} /> MCP servers
            </h2>
            <p className="mb-3 text-[11px] text-neutral-500">
              Model Context Protocol servers give the agent extra tools: a browser (Playwright), your Laravel app
              (Laravel Boost), hosted services. Servers run in the open project; the agent asks before each tool call
              unless you allow MCP tools for the session or use full access. Not available in Plan and Review modes.
            </p>
            <McpSettings />
          </section>

          {/* Agent CLIs */}
          <section id="agent-clis">
            <h2 className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Bot size={13} /> Agent CLIs
            </h2>
            <p className="mb-3 text-[11px] text-neutral-500">
              Shown in the + menu of the chat column. Each command runs in a terminal tab through your login
              shell, so it uses your PATH and the CLI's own login. Bypass flags are added only when you choose
              "bypass" in the menu; the agent then runs without asking before edits or commands.
            </p>
            <AgentCliSettings />
          </section>

          {/* Permissions */}
          <section>
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <ShieldCheck size={13} /> Default permission policy
            </h2>
            <Field
              label="Policy for new sessions"
              hint="Applies to new sessions; per-session policy can be changed in Agent Studio profiles."
            >
              <select
                className={inputClass}
                value={settings.default_policy_mode}
                onChange={(e) =>
                  void updateSettings({ default_policy_mode: e.target.value as "safe" | "balanced" | "yolo" })
                }
              >
                <option value="safe">Safe — ask before writes, deletions, commands</option>
                <option value="balanced">Balanced — auto-apply file edits</option>
                <option value="yolo">Yolo — auto-approve inside workspace</option>
              </select>
            </Field>
            <Field
              label="Max agent steps per turn"
              hint="How many model ↔ tool round-trips one message may take. When reached, the agent pauses and you can press Continue."
            >
              <input
                type="number"
                min={1}
                max={500}
                className={`${inputClass} w-32`}
                value={settings.max_iterations}
                onChange={(e) =>
                  void updateSettings({ max_iterations: Math.min(500, Math.max(1, Number(e.target.value) || 1)) })
                }
              />
            </Field>
            <Field
              label="Language-server diagnostics"
              hint="After the agent edits a TypeScript/JavaScript, PHP, Python, Rust or Go file, errors from a language server are sent back to it so it can fix them."
            >
              <select
                className={inputClass}
                value={settings.lsp_diagnostics ? "on" : "off"}
                onChange={(e) => void updateSettings({ lsp_diagnostics: e.target.value === "on" })}
              >
                <option value="on">On (recommended)</option>
                <option value="off">Off</option>
              </select>
            </Field>
            {settings.lsp_diagnostics && (
              <Field
                label="Missing language servers"
                hint="Servers on your PATH are used first. Otherwise vtsls, intelephense or pyright can be downloaded and run with npx (needs Node.js); Rust and Go need rust-analyzer / gopls installed."
              >
                <select
                  className={inputClass}
                  value={settings.lsp_auto_download ? "download" : "skip"}
                  onChange={(e) => void updateSettings({ lsp_auto_download: e.target.value === "download" })}
                >
                  <option value="download">Download with npx when needed</option>
                  <option value="skip">Only use installed servers</option>
                </select>
              </Field>
            )}
          </section>

          {/* Shortcuts */}
          <section>
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Keyboard size={13} /> Keyboard shortcuts
            </h2>
            <div className="rounded-md border border-neutral-200 p-4 dark:border-neutral-800">
              {SHORTCUTS.map(([k, v]) => (
                <div key={k} className="flex items-center justify-between py-1 text-xs">
                  <span className="text-neutral-500">{v}</span>
                  <kbd className="rounded border border-neutral-300 px-1.5 py-0.5 font-mono text-[10px] dark:border-neutral-700">
                    {k}
                  </kbd>
                </div>
              ))}
            </div>
          </section>

          {/* Privacy & data */}
          <section>
            <h2 className="mb-3 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-neutral-500">
              <Database size={13} /> Privacy &amp; data
            </h2>
            <div className="space-y-3 rounded-md border border-neutral-200 p-4 text-xs leading-relaxed text-neutral-500 dark:border-neutral-800">
              <p className="flex gap-2">
                <EyeOff size={14} className="mt-0.5 shrink-0" />
                <span>
                  Your project files stay on this machine. Only the conversation context (your
                  messages, attached files, and file contents the agent reads) is sent to the
                  AI provider you selected — directly from this app, with no JenderalCode server
                  in between.
                </span>
              </p>
              <p className="flex gap-2">
                <Coins size={14} className="mt-0.5 shrink-0" />
                <span>
                  API keys are stored outside the database and are never written to logs — by
                  default in a file only your user can read (permission 0600), so using the app
                  never asks for your computer password. You can move them to the OS keychain
                  (macOS Keychain / Windows Credential Manager / Secret Service) below.
                  {credBackend && (
                    <>
                      {" "}
                      Current storage:{" "}
                      <b>{credBackend === "os-keychain" ? "OS keychain" : "local file (0600)"}</b>.
                    </>
                  )}
                </span>
              </p>
              <Field
                label="API key storage"
                hint="Local file: no password prompts (recommended for this build). OS keychain: strongest isolation, but unsigned builds may ask for your login password once per app launch."
              >
                <div className="flex items-center gap-2">
                  <Button
                    size="sm"
                    variant={settings.credential_storage !== "keychain" ? "primary" : "secondary"}
                    onClick={() => void switchCredStorage("file")}
                  >
                    Local file (0600)
                  </Button>
                  <Button
                    size="sm"
                    variant={settings.credential_storage === "keychain" ? "primary" : "secondary"}
                    onClick={() => void switchCredStorage("keychain")}
                  >
                    OS keychain
                  </Button>
                </div>
              </Field>
              <p className="flex gap-2">
                <Database size={14} className="mt-0.5 shrink-0" />
                <span>
                  Chat history, tool runs, and usage are stored locally in SQLite. You can export
                  or delete them at any time below.
                  {dataDir && (
                    <>
                      {" "}
                      Data directory: <code className="font-mono">{dataDir}</code>
                    </>
                  )}
                </span>
              </p>
            </div>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button
                onClick={async () => {
                  const target = await saveDialog({
                    title: "Export history",
                    defaultPath: "jenderalcode-history.json",
                    filters: [{ name: "JSON", extensions: ["json"] }],
                  });
                  if (typeof target === "string") {
                    await api.exportHistory(target);
                    setMessage(`Exported to ${target}`);
                  }
                }}
              >
                <Download size={12} /> Export history
              </Button>
              <Field label="">
                <div className="flex items-center gap-2">
                  <input
                    type="number"
                    min={1}
                    className={`${inputClass} w-28`}
                    placeholder="days"
                    onChange={(e) => localStorage.setItem("jcode:retention-days", e.target.value)}
                  />
                  <Button
                    onClick={async () => {
                      const days = parseInt(localStorage.getItem("jcode:retention-days") ?? "30", 10);
                      if (!Number.isFinite(days) || days < 1) return;
                      if (!await confirmAction(`Delete sessions not touched in the last ${days} days?`)) return;
                      const n = await api.applyRetention(days);
                      await updateSettings({ retention_days: days });
                      setMessage(`Deleted ${n} session(s) older than ${days} days.`);
                      void refreshUsage();
                    }}
                  >
                    Apply retention
                  </Button>
                </div>
              </Field>
              <Button
                variant="danger"
                onClick={async () => {
                  if (!await confirmAction("Delete ALL session history? This cannot be undone.")) return;
                  const n = await api.clearHistory();
                  setMessage(`Deleted ${n} session(s).`);
                  void refreshUsage();
                }}
              >
                <Trash2 size={12} /> Clear all history
              </Button>
            </div>
            {message && <div className="mt-2 text-[11px] text-emerald-500">{message}</div>}
          </section>
        </div>
      </div>
    </div>
  );
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-neutral-200 p-3 dark:border-neutral-800">
      <div className="text-[10px] font-medium uppercase tracking-wide text-neutral-500">{label}</div>
      <div className="mt-1 text-lg font-semibold">{value}</div>
    </div>
  );
}
