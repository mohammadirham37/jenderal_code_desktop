/** Skills: search and install from skills.sh, turn installed skills on/off
 *  for the agent, and remove skills installed here. */
import { useEffect, useState } from "react";
import { Download, Search, Trash2 } from "lucide-react";
import { api, type SkillSearchResult } from "@/lib/ipc";
import { useApp } from "@/stores/app";
import { useSkills } from "@/stores/skills";
import { Button, IconButton, Spinner, Toggle, inputClass } from "@/components/ui";
import { formatTokens } from "@/lib/format";
import { confirmAction } from "@/lib/confirm";

export function SkillsSettings() {
  const skills = useSkills((s) => s.skills);
  const disabled = useApp((s) => s.settings.disabled_skills);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SkillSearchResult[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; error: boolean } | null>(null);

  useEffect(() => {
    void useSkills.getState().load();
  }, []);

  const installed = new Set(skills?.map((s) => s.name) ?? []);

  const search = async () => {
    if (!query.trim()) return;
    setSearching(true);
    setMessage(null);
    try {
      setResults(await api.searchSkills(query));
    } catch (e) {
      setMessage({ text: String(e), error: true });
    } finally {
      setSearching(false);
    }
  };

  const run = async (key: string, action: () => Promise<string>, done: string) => {
    setBusy(key);
    setMessage(null);
    try {
      await action();
      await useSkills.getState().load();
      setMessage({ text: done, error: false });
    } catch (e) {
      setMessage({ text: String(e), error: true });
    } finally {
      setBusy(null);
    }
  };

  const toggle = (name: string, enabled: boolean) =>
    void useApp.getState().updateSettings({
      disabled_skills: enabled ? disabled.filter((n) => n !== name) : [...disabled, name],
    });

  return (
    <div className="space-y-4">
      <div>
        <div className="flex gap-2">
          <div className="relative min-w-0 flex-1">
            <Search size={12} className="absolute left-2.5 top-2 text-neutral-400" />
            <input
              className={`${inputClass} pl-7`}
              value={query}
              placeholder="Search skills.sh (e.g. brainstorming, frontend design, testing)"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void search()}
            />
          </div>
          <Button size="sm" onClick={() => void search()} disabled={searching || !query.trim()}>
            {searching ? <Spinner className="h-3 w-3" /> : "Search"}
          </Button>
        </div>
        {message && (
          <div className={`mt-2 whitespace-pre-wrap text-[11px] ${message.error ? "text-red-500" : "text-accent-600 dark:text-accent-400"}`}>
            {message.text}
          </div>
        )}
        {results && (
          <div className="mt-2 max-h-72 overflow-y-auto rounded-[5px] border border-neutral-200 dark:border-neutral-800">
            {results.length === 0 && <div className="px-3 py-3 text-[11px] text-neutral-500">No skills found.</div>}
            {results.map((r) => {
              const key = `${r.source}@${r.skill}`;
              const has = installed.has(r.skill);
              return (
                <div key={key} className="flex items-center gap-2 border-b border-neutral-200 px-3 py-2 last:border-0 dark:border-neutral-800">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs font-medium">{r.skill}</div>
                    <div className="truncate font-mono text-[10px] text-neutral-500">
                      {r.source} · {formatTokens(r.installs)} installs
                    </div>
                  </div>
                  <Button
                    size="sm"
                    disabled={has || busy !== null}
                    onClick={() => void run(key, () => api.installSkill(r.source, r.skill), `Installed ${r.skill}.`)}
                  >
                    {busy === key ? <Spinner className="h-3 w-3" /> : has ? "Installed" : (
                      <>
                        <Download size={11} /> Install
                      </>
                    )}
                  </Button>
                </div>
              );
            })}
          </div>
        )}
        <p className="mt-1.5 text-[10.5px] text-neutral-500">
          Installs with <code className="font-mono">npx skills add … -g -a universal</code> into ~/.agents/skills. Skills from
          ~/.claude, ~/.codex, ~/.config/opencode and the project&apos;s .agents/.claude skills folders are picked up too.
        </p>
      </div>

      <div>
        <div className="eyebrow mb-1.5">Installed ({skills?.length ?? "…"})</div>
        {skills === null && <Spinner />}
        <div className="space-y-px">
          {skills?.map((s) => (
            <div key={s.name} className="flex items-start gap-2 rounded-[4px] px-2 py-1.5 hover:bg-neutral-100 dark:hover:bg-neutral-800/50">
              <div className="pt-0.5">
                <Toggle checked={s.enabled} onChange={(v) => toggle(s.name, v)} />
              </div>
              <div className="min-w-0 flex-1" title={s.path}>
                <div className="flex items-baseline gap-1.5">
                  <span className={`text-xs font-medium ${s.enabled ? "" : "text-neutral-500"}`}>{s.name}</span>
                  {s.project && <span className="font-mono text-[10px] text-accent-600 dark:text-accent-400">project</span>}
                </div>
                <div className="line-clamp-2 text-[11px] text-neutral-500">{s.description}</div>
              </div>
              {s.removable && (
                <IconButton
                  title="Remove skill"
                  disabled={busy !== null}
                  onClick={async () => {
                    if (await confirmAction(`Remove skill "${s.name}"?`)) void run(`rm-${s.name}`, () => api.removeSkill(s.name), `Removed ${s.name}.`);
                  }}
                >
                  {busy === `rm-${s.name}` ? <Spinner className="h-3 w-3" /> : <Trash2 size={12} />}
                </IconButton>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
