import {
  rebaseSkillSelection,
  skillsForProvider,
  type ResolvedSkillSelection,
} from "./skill-tokens";
import { useSkillsStore } from "@/stores/skills-store";
import { listSkills, type Skill } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/** Refresh a selected draft inventory after deferred worktree creation and
 * translate path-derived ids to the new checkout without name-first lookup. */
export async function refreshSkillSelectionForCwd(
  selection: ResolvedSkillSelection,
  sourceSkills: Skill[],
  sourceCwd: string,
  targetCwd: string,
  provider: AgentChatProviderKind,
): Promise<ResolvedSkillSelection> {
  if (sourceCwd === targetCwd || selection.skillIds.length === 0) {
    return selection;
  }
  const state = useSkillsStore.getState();
  const response = await listSkills(targetCwd, state.includePlugins, true);
  const inventory = Array.isArray(response) ? response : response.skills;
  const enabled = inventory.filter(
    (skill) => !state.disabledIds.includes(skill.preferenceId ?? skill.id),
  );
  return rebaseSkillSelection(
    selection,
    sourceSkills,
    sourceCwd,
    targetCwd,
    skillsForProvider(enabled, provider),
  );
}
