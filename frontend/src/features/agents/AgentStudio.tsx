/** Agent Studio: create and edit agent profiles (prompt, tools, policy). */
import { useEffect, useState } from "react";
import { Bot, Lock, Pencil, Plus, ShieldCheck, Trash2, Wrench } from "lucide-react";
import { useChat } from "@/stores/chat";
import { Badge, Button, EmptyState, Field, Modal, inputClass } from "@/components/ui";
import type { AgentProfile, PolicyMode } from "@/lib/ipc";

const ALL_TOOLS = [
  "fs_read",
  "fs_list",
  "fs_search",
  "glob_files",
  "fs_write",
  "fs_edit",
  "fs_delete",
  "terminal_run",
  "git_status",
  "git_diff",
  "git_log",
];

const POLICY_HINT: Record<PolicyMode, string> = {
  safe: "Ask before every file write, deletion, and command (recommended).",
  balanced: "File edits inside the workspace apply immediately; commands and deletions still ask.",
  yolo: "Always allow: no prompts at all (including git push and sensitive files). Catastrophic commands and writes outside the workspace remain blocked.",
};

const emptyProfile = (): AgentProfile => ({
  id: crypto.randomUUID(),
  name: "",
  mode: "custom",
  system_prompt: "",
  allowed_tools: null,
  policy_mode: "safe",
  provider_id: null,
  model_id: null,
  built_in: false,
  created_at: 0,
});

export function AgentStudio() {
  const profiles = useChat((s) => s.profiles);
  const { loadProfiles, saveAgentProfile, deleteAgentProfile } = useChat.getState();
  const [editing, setEditing] = useState<AgentProfile | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void loadProfiles();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async () => {
    if (!editing) return;
    if (!editing.name.trim()) {
      setError("Name is required.");
      return;
    }
    await saveAgentProfile(editing);
    setEditing(null);
    setError(null);
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between border-b border-neutral-200 px-4 dark:border-neutral-800">
        <h1 className="flex items-center gap-2 text-sm font-semibold">
          <Bot size={15} className="text-indigo-400" /> Agent Studio
        </h1>
        <Button
          variant="primary"
          size="sm"
          onClick={() => {
            setEditing(emptyProfile());
            setError(null);
          }}
        >
          <Plus size={12} /> New profile
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {profiles.length === 0 && (
          <EmptyState icon={<Bot size={30} />} title="No agent profiles" />
        )}
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {profiles.map((p) => (
            <div key={p.id} className="rounded-xl border border-neutral-200 p-4 dark:border-neutral-800">
              <div className="mb-1.5 flex items-center gap-2">
                <span className="text-sm font-medium">{p.name}</span>
                <Badge tone={p.mode === "build" ? "indigo" : p.mode === "plan" ? "blue" : p.mode === "review" ? "amber" : "neutral"}>
                  {p.mode}
                </Badge>
                {p.built_in && (
                  <Badge tone="neutral">
                    <Lock size={8} /> built-in
                  </Badge>
                )}
              </div>
              <p className="mb-2 line-clamp-3 text-[11px] leading-relaxed text-neutral-500">
                {p.system_prompt}
              </p>
              <div className="mb-3 flex flex-wrap items-center gap-1 text-[10px] text-neutral-500">
                <ShieldCheck size={10} /> {p.policy_mode}
                <span>·</span>
                <Wrench size={10} />
                {p.allowed_tools ? `${p.allowed_tools.length} tools` : "all tools"}
              </div>
              <div className="flex gap-1.5">
                <Button
                  size="sm"
                  onClick={() => {
                    setEditing({ ...p });
                    setError(null);
                  }}
                >
                  <Pencil size={11} /> Edit
                </Button>
                {!p.built_in && (
                  <Button
                    variant="danger"
                    size="sm"
                    onClick={async () => {
                      if (window.confirm(`Delete profile ${p.name}?`)) await deleteAgentProfile(p.id);
                    }}
                  >
                    <Trash2 size={11} />
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>

      <Modal open={editing !== null} title={editing?.built_in ? `Edit ${editing.name}` : "Agent profile"} onClose={() => setEditing(null)} wide>
        {editing && (
          <>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Name">
                <input
                  className={inputClass}
                  value={editing.name}
                  onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                />
              </Field>
              <Field label="Mode">
                <select
                  className={inputClass}
                  value={editing.mode}
                  onChange={(e) => setEditing({ ...editing, mode: e.target.value as AgentProfile["mode"] })}
                >
                  <option value="build">Build — implement features</option>
                  <option value="plan">Plan — read-only planning</option>
                  <option value="review">Review — read-only inspection</option>
                  <option value="custom">Custom</option>
                </select>
              </Field>
            </div>
            <Field label="System prompt" hint="Appended with workspace context (file tree, git status, project instructions).">
              <textarea
                className={`${inputClass} h-40 resize-y font-mono`}
                value={editing.system_prompt}
                onChange={(e) => setEditing({ ...editing, system_prompt: e.target.value })}
              />
            </Field>
            <Field label="Permission policy" hint={POLICY_HINT[editing.policy_mode]}>
              <select
                className={inputClass}
                value={editing.policy_mode}
                onChange={(e) => setEditing({ ...editing, policy_mode: e.target.value as PolicyMode })}
              >
                <option value="safe">Safe — ask for everything</option>
                <option value="balanced">Balanced — auto-apply file edits</option>
                <option value="yolo">Yolo — auto-approve in workspace</option>
              </select>
            </Field>
            <Field label="Tools" hint="Uncheck all to allow every tool.">
              <div className="flex flex-wrap gap-1">
                {ALL_TOOLS.map((t) => {
                  const all = !editing.allowed_tools;
                  const checked = all || editing.allowed_tools!.includes(t);
                  return (
                    <button
                      key={t}
                      onClick={() => {
                        let next: string[] | null;
                        if (all) {
                          next = ALL_TOOLS.filter((x) => x !== t);
                        } else {
                          next = checked
                            ? editing.allowed_tools!.filter((x) => x !== t)
                            : [...editing.allowed_tools!, t];
                          if (next.length === ALL_TOOLS.length) next = null;
                        }
                        setEditing({ ...editing, allowed_tools: next });
                      }}
                      className={`rounded-md border px-2 py-1 font-mono text-[10px] ${
                        checked
                          ? "border-indigo-500 bg-indigo-500/10 text-indigo-400"
                          : "border-neutral-300 text-neutral-500 dark:border-neutral-700"
                      }`}
                    >
                      {t}
                    </button>
                  );
                })}
              </div>
            </Field>
            {error && <div className="mb-2 text-[11px] text-red-500">{error}</div>}
            <div className="mt-2 flex justify-end gap-2">
              <Button onClick={() => setEditing(null)}>Cancel</Button>
              <Button variant="primary" onClick={() => void save()}>
                Save profile
              </Button>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
