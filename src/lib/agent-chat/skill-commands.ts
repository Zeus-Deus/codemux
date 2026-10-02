import { BookOpen } from "lucide-react";

import type { Skill } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";
import { sessionProviderLabel } from "./session-mentions";

import { skillTokenFor } from "./skill-tokens";
import type { SlashCommandItem } from "./slash-commands";

interface BuildSkillCommandsArgs {
  skills: Skill[];
  providerCommands?: readonly { name: string }[];
  activeProvider?: AgentChatProviderKind;
  /** Called when the user activates an exact skill definition. */
  onInvoke: (skill: Skill) => void;
}

/**
 * Build the `SKILLS` group of slash items.
 *
 * Items are returned in the order the backend produced them
 * (provider → scope → name) so the popup mirrors the order the user
 * sees in Settings.
 *
 * Same-named definitions remain separate. Their command text is qualified
 * by provider and scope, and the backend receives the selected stable id.
 */
export function buildSkillCommands({
  skills,
  providerCommands = [],
  activeProvider,
  onInvoke,
}: BuildSkillCommandsArgs): SlashCommandItem[] {
  const idCounts = new Map<string, number>();
  for (const skill of skills) {
    idCounts.set(skill.id, (idCounts.get(skill.id) ?? 0) + 1);
  }
  return skills.map((skill, index) => ({
    id: `skill:${skill.id}${(idCounts.get(skill.id) ?? 0) > 1 ? `:${index}` : ""}`,
    label: skill.name,
    description: formatSkillDescription(skill),
    // Search the skill's own prose, not the `provider · scope` suffix,
    // or `/codex` would match every Codex skill.
    searchDescription: skill.description ?? "",
    command: providerCommands.some((command) => command.name.toLowerCase() === skillTokenFor(skill, skills).slice(1).toLowerCase())
      ? skillTokenFor(skill, skills).replace(/^\//, "$") : skillTokenFor(skill, skills),
    icon: BookOpen,
    identity: { provider: skill.provider, kind: "skill", label: skillIdentityLabel(skill, activeProvider) },
    group: "SKILLS",
    onSelect: () => onInvoke(skill),
  }));
}

function skillIdentityLabel(skill: Skill, activeProvider?: AgentChatProviderKind): string {
  const scope = skill.scope === "plugin" && skill.pluginSlug ? `plugin/${skill.pluginSlug}` : skill.scope;
  const source = `${sessionProviderLabel(skill.provider)} skill · ${scope}`;
  if (!activeProvider) return source;
  const availability = skill.projections?.find((projection) => projection.targetProvider === activeProvider)?.availability;
  if (availability === "native") return `${source} · Native in ${sessionProviderLabel(activeProvider)}`;
  if (availability === "explicit-portable" || (!skill.projections && skill.provider !== activeProvider)) return `${source} · Portable in ${sessionProviderLabel(activeProvider)}`;
  return source;
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
