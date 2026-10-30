/** Provider Center: manage AI providers, keys, models, connection tests. */
import { useEffect, useState } from "react";
import {
  Cloud,
  Cpu,
  KeyRound,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  Zap,
} from "lucide-react";
import { useProviders } from "@/stores/providers";
import {
  Badge,
  Button,
  EmptyState,
  Field,
  Modal,
  Spinner,
  Toggle,
  inputClass,
} from "@/components/ui";
import type { ModelInfo, ProviderConfig, ProviderKind } from "@/lib/ipc";
import { confirmAction } from "@/lib/confirm";

const KIND_LABEL: Record<ProviderKind, string> = {
  openai_compatible: "OpenAI-compatible",
  anthropic: "Anthropic",
  google: "Google Gemini",
  mock: "Mock (local)",
  external_cli: "External CLI",
};

interface Draft {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  enabled: boolean;
  created_at: number;
  updated_at: number;
  extra?: unknown;
  credential_ref?: string | null;
}

const emptyDraft = (): Draft => ({
  id: crypto.randomUUID(),
  name: "",
  kind: "openai_compatible",
  base_url: "",
  enabled: true,
  created_at: 0,
  updated_at: 0,
  extra: {},
});

export function ProviderCenter() {
  const store = useProviders();
  const [editing, setEditing] = useState<Draft | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [customModel, setCustomModel] = useState("");
  const [customPriceIn, setCustomPriceIn] = useState("");
  const [customPriceOut, setCustomPriceOut] = useState("");
  const [modelsFor, setModelsFor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (store.providers.length === 0 && !store.loading) void store.load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async () => {
    if (!editing) return;
    if (!editing.name.trim()) {
      setError("Name is required.");
      return;
    }
    if (editing.kind !== "mock" && !editing.base_url.trim()) {
      setError("Base URL is required.");
      return;
    }
    try {
      await store.save(editing as ProviderConfig, apiKey || undefined);
      setEditing(null);
      setApiKey("");
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-14 shrink-0 items-center justify-between border-b border-neutral-200 px-4 dark:border-neutral-800">
        <h1 className="flex items-center gap-2.5 font-display text-[19px] font-semibold tracking-tight">
          <Plug size={16} className="text-accent-500" /> Provider Center
        </h1>
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            setEditing(emptyDraft());
            setApiKey("");
            setError(null);
          }}
        >
          <Plus size={12} /> Add provider
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {store.providers.length === 0 && (
          <EmptyState
            icon={<Cloud size={30} />}
            title="No providers configured"
            hint="Add OpenAI, Anthropic, Google Gemini, a local runtime, or any OpenAI-compatible endpoint. The built-in Mock provider appears automatically for offline demos."
          />
        )}

        <div className="grid gap-3 md:grid-cols-2">
          {store.providers.map((p) => {
            const models = store.models.filter((m) => m.provider_id === p.id);
            const test = store.testResults[p.id];
            return (
              <div
                key={p.id}
                className="rounded-md border border-neutral-200 p-4 dark:border-neutral-800"
              >
                <div className="mb-2 flex items-start justify-between gap-2">
                  <div className="flex items-center gap-2">
                    {p.kind === "mock" ? (
                      <Cpu size={15} className="text-emerald-400" />
                    ) : (
                      <Server size={15} className="text-accent-600 dark:text-accent-400" />
                    )}
                    <div>
                      <div className="flex items-center gap-2 text-sm font-medium">
                        {p.name}
                        <Badge tone={p.enabled ? "green" : "neutral"}>
                          {p.enabled ? "enabled" : "disabled"}
                        </Badge>
                        {p.credential_ref && (
                          <Badge tone="blue">
                            <KeyRound size={8} /> key stored
                          </Badge>
                        )}
                      </div>
                      <div className="text-[11px] text-neutral-500">
                        {KIND_LABEL[p.kind]}
                        {p.base_url ? ` · ${p.base_url}` : ""}
                      </div>
                    </div>
                  </div>
                  <Toggle checked={p.enabled} onChange={(v) => void store.save({ ...p, enabled: v })} />
                </div>

                <div className="mb-3 flex flex-wrap items-center gap-1.5">
                  <Button
                    size="sm"
                    onClick={() => void store.test(p.id)}
                    disabled={store.testing[p.id]}
                  >
                    {store.testing[p.id] ? <Spinner /> : <Zap size={11} />} Test & refresh
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => {
                      setEditing({ ...p });
                      setApiKey("");
                      setError(null);
                    }}
                  >
                    <Pencil size={11} /> Edit
                  </Button>
                  <Button
                    size="sm"
                    onClick={() => {
                      setModelsFor(modelsFor === p.id ? null : p.id);
                      setCustomModel("");
                    }}
                  >
                    Models ({models.length})
                  </Button>
                  {p.kind !== "mock" && (
                    <Button
                      variant="danger"
                      size="sm"
                      onClick={async () => {
                        if (await confirmAction(`Delete provider ${p.name}? Its stored key will be removed too.`))
                          await store.remove(p.id);
                      }}
                    >
                      <Trash2 size={11} />
                    </Button>
                  )}
                </div>

                {test && (
                  <div
                    className={`mb-2 rounded-md px-2 py-1.5 text-[11px] ${test.ok ? "bg-emerald-500/10 text-emerald-400" : "bg-red-500/10 text-red-400"}`}
                  >
                    {test.ok
                      ? `✓ Connected in ${test.latency_ms}ms · ${test.model_count} model(s) available`
                      : `✗ ${test.error}`}
                  </div>
                )}

                {modelsFor === p.id && (
                  <div className="rounded-md border border-neutral-200 p-2 dark:border-neutral-800">
                    <div className="max-h-44 overflow-y-auto">
                      {models.map((m) => (
                        <ModelRow key={m.id} model={m} onDelete={() => void store.deleteModel(m.id)} />
                      ))}
                      {models.length === 0 && (
                        <div className="py-2 text-center text-[11px] text-neutral-500">
                          No models — run “Test & refresh” or add one manually.
                        </div>
                      )}
                    </div>
                    <div className="mt-2 flex gap-1">
                      <input
                        className={inputClass}
                        placeholder="model name, e.g. gpt-4o-mini"
                        value={customModel}
                        onChange={(e) => setCustomModel(e.target.value)}
                      />
                      <input
                        className={`${inputClass} w-24`}
                        placeholder="$/Min"
                        value={customPriceIn}
                        onChange={(e) => setCustomPriceIn(e.target.value)}
                        title="Input price per 1M tokens (USD, optional)"
                      />
                      <input
                        className={`${inputClass} w-24`}
                        placeholder="$/Mout"
                        value={customPriceOut}
                        onChange={(e) => setCustomPriceOut(e.target.value)}
                        title="Output price per 1M tokens (USD, optional)"
                      />
                      <Button
                        size="sm"
                        onClick={async () => {
                          if (!customModel.trim()) return;
                          await store.addCustomModel(
                            p.id,
                            customModel.trim(),
                            undefined,
                            parseFloat(customPriceIn) || undefined,
                            parseFloat(customPriceOut) || undefined,
                          );
                          setCustomModel("");
                        }}
                      >
                        Add
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* Editor modal */}
      <Modal
        open={editing !== null}
        title={editing?.created_at ? "Edit provider" : "Add provider"}
        onClose={() => setEditing(null)}
      >
        {editing && (
          <>
            <Field label="Preset">
              <div className="flex flex-wrap gap-1">
                {store.presets.map((preset) => (
                  <button
                    key={preset.id}
                    onClick={() => {
                      setEditing({
                        ...editing,
                        name: preset.label,
                        kind: preset.kind,
                        base_url: preset.base_url,
                      });
                    }}
                    className={`rounded-md border px-2 py-1 text-[11px] ${
                      editing.name === preset.label
                        ? "border-accent-500 bg-accent-500/10 text-accent-600 dark:text-accent-400"
                        : "border-neutral-300 hover:border-neutral-400 dark:border-neutral-700"
                    }`}
                  >
                    {preset.label}
                  </button>
                ))}
              </div>
            </Field>
            <Field label="Name">
              <input
                className={inputClass}
                value={editing.name}
                onChange={(e) => setEditing({ ...editing, name: e.target.value })}
              />
            </Field>
            <Field label="Adapter type">
              <select
                className={inputClass}
                value={editing.kind}
                onChange={(e) => setEditing({ ...editing, kind: e.target.value as ProviderKind })}
              >
                <option value="openai_compatible">OpenAI-compatible</option>
                <option value="anthropic">Anthropic</option>
                <option value="google">Google Gemini</option>
                <option value="mock">Mock</option>
              </select>
            </Field>
            {editing.kind !== "mock" && (
              <Field label="Base URL" hint="e.g. https://api.openai.com/v1 (local: http://localhost:11434/v1)">
                <input
                  className={inputClass}
                  value={editing.base_url}
                  onChange={(e) => setEditing({ ...editing, base_url: e.target.value })}
                />
              </Field>
            )}
            <Field
              label="API key"
              hint={
                editing.credential_ref
                  ? "A key is already stored (see Settings → Privacy for the storage location). Type a new one to replace it; empty to remove."
                  : "Stored outside the database — in the local credentials file or the OS keychain, per Settings → Privacy. Never in logs."
              }
            >
              <input
                type="password"
                className={inputClass}
                value={apiKey}
                placeholder={editing.credential_ref ? "•••••• (unchanged)" : "sk-…"}
                onChange={(e) => setApiKey(e.target.value)}
              />
            </Field>
            {error && <div className="mb-2 text-[11px] text-red-500">{error}</div>}
            <div className="mt-2 flex justify-end gap-2">
              <Button onClick={() => setEditing(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => void save()}>
                Save provider
              </Button>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}

function ModelRow({ model, onDelete }: { model: ModelInfo; onDelete: () => void }) {
  const caps = model.capabilities;
  return (
    <div className="group flex items-center gap-2 px-1 py-1 text-[11px]">
      <RefreshCw size={9} className={model.custom ? "text-amber-400" : "text-neutral-600"} />
      <span className="min-w-0 flex-1 truncate font-mono">{model.name}</span>
      {caps.tool_calling && <Badge tone="accent">tools</Badge>}
      {caps.vision && <Badge tone="blue">vision</Badge>}
      {model.custom && <Badge tone="amber">custom</Badge>}
      <button
        className="hidden text-neutral-500 hover:text-red-500 group-hover:block"
        onClick={onDelete}
      >
        <Trash2 size={10} />
      </button>
    </div>
  );
}
