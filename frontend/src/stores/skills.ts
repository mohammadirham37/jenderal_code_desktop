/** Installed skills (SKILL.md folders) for the Skills settings and the
 *  composer's `/` menu. Reloaded when the project or the skill set changes. */
import { create } from "zustand";
import { api, type SkillInfo } from "@/lib/ipc";
import { useApp } from "./app";

interface SkillsState {
  skills: SkillInfo[] | null;
  load: () => Promise<void>;
}

export const useSkills = create<SkillsState>((set) => ({
  skills: null,
  load: async () => {
    try {
      set({ skills: await api.listSkills() });
    } catch {
      set({ skills: [] });
    }
  },
}));

useApp.subscribe((s, prev) => {
  if (s.workspace?.id !== prev.workspace?.id || s.settings.disabled_skills !== prev.settings.disabled_skills) {
    if (useSkills.getState().skills !== null) void useSkills.getState().load();
  }
});
