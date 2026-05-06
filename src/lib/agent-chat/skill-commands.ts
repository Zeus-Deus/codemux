import { BookOpen } from "lucide-react";

import type { Skill } from "@/tauri/commands";

import type { SlashCommandItem } from "./slash-commands";

interface BuildSkillCommandsArgs {
  skills: Skill[];
  /**
   * Called when the user activates a skill in the popup. Stage 2 wires
   * a stub that toasts; Stage 3 will stage the skill as a chip on the
   * composer for prompt-prefix injection on send.
   */
  onInvoke: (skill: Skill) => void;
}

/**
 * Build the `SKILLS` group of slash items.
 *
 * Items are returned in the order the backend produced them
 * (provider → scope → name) so the popup mirrors the order the user
 * sees in Settings.
 */
export function buildSkillCommands({
  skills,
  onInvoke,
}: BuildSkillCommandsArgs): SlashCommandItem[] {
  return skills.map((skill) => ({
    id: `skill:${skill.id}`,
    label: skill.name,
    description: formatSkillDescription(skill),
    command: `/${skill.name}`,
    icon: BookOpen,
    group: "SKILLS",
    onSelect: () => onInvoke(skill),
  }));
}

/**
 * One-line description for the popup row. Combines the skill's own
 * description with a muted scope indicator (`provider · scope`) so the
 * user can disambiguate same-name skills coming from different sources.
 */
export function formatSkillDescription(skill: Skill): string {
  const scope = formatScopeIndicator(skill);
  if (!skill.description) return scope;
  return `${skill.description} · ${scope}`;
}

/**
 * Compact source indicator. Plugin-bundled skills include the plugin
 * slug so the user can tell `/release-codemux` from a marketplace
 * lookalike.
 */
export function formatScopeIndicator(skill: Skill): string {
  if (skill.scope === "plugin" && skill.pluginSlug) {
    return `${skill.provider} · plugin/${skill.pluginSlug}`;
  }
  return `${skill.provider} · ${skill.scope}`;
}
