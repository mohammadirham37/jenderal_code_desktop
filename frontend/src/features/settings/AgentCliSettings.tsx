/** Edit the agent CLIs shown in the chat column's "+" menu. Text fields save
 *  on blur; toggles, removals and additions save immediately. */
import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import type { AgentCli } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { Button, IconButton, Toggle, inputClass } from "@/components/ui";

export function AgentCliSettings() {
  const agents = useApp((s) => s.settings.agent_clis);
  const [draft, setDraft] = useState<AgentCli[]>(agents);
  useEffect(() => setDraft(agents), [agents]);

  const save = (next: AgentCli[]) => void useApp.getState().updateSettings({ agent_clis: next });
  const edit = (i: number, p: Partial<AgentCli>) =>
    setDraft((d) => d.map((a, j) => (j === i ? { ...a, ...p } : a)));

  return (
    <div className="space-y-1.5">
      {draft.map((a, i) => (
        <div key={a.id} className="flex items-center gap-2">
          <Toggle
            checked={a.enabled}
            onChange={(enabled) => save(draft.map((x, j) => (j === i ? { ...x, enabled } : x)))}
          />
          <div className="w-36 shrink-0">
            <input
              className={inputClass}
              value={a.name}
              placeholder="Name"
              onChange={(e) => edit(i, { name: e.target.value })}
              onBlur={() => save(draft)}
            />
          </div>
          <div className="min-w-0 flex-1">
            <input
              className={`${inputClass} font-mono`}
              value={a.command}
              placeholder="command --flags"
              onChange={(e) => edit(i, { command: e.target.value })}
              onBlur={() => save(draft)}
            />
          </div>
          <IconButton title="Remove" onClick={() => save(draft.filter((_, j) => j !== i))}>
            <Trash2 size={12} />
          </IconButton>
        </div>
      ))}
      <Button
        size="sm"
        onClick={() =>
          save([...draft, { id: `custom-${Date.now().toString(36)}`, name: "New agent", command: "", enabled: true }])
        }
      >
        <Plus size={12} /> Add agent
      </Button>
    </div>
  );
}
