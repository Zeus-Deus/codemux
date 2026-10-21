// Shape skills for the Settings → Skills list. The backend returns one
// entry per place a skill was found, and the same SKILL.md is commonly
// reachable from several providers (`~/.claude/skills`, `~/.agents/skills`
// and `~/.codemux/skills` are often symlinks to one source, or identical
// copies). Settings shows one row per skill name, tagged with every
// source that has it, and only splits a row into versions when the
// content actually differs.

import type { Skill, SkillProvider, SkillScope } from "@/tauri/commands";

const PROVIDER_LABEL: Record<SkillProvider, string> = {
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
  codemux: "Codemux",
};

const SCOPE_LABEL: Partial<Record<SkillScope, string>> = {
  project: "Project",
  managed: "Managed",
  admin: "Admin",
  system: "System",
};

/** Short tag naming where one copy of a skill comes from. User-wide and
 *  provider-configured skills are the common case and show the provider
 *  alone; plugin skills collapse into "Plugin" whatever their provider. */
export function skillSourceLabel(skill: Skill): string {
  if (skill.scope === "plugin") return "Plugin";
  const scope = SCOPE_LABEL[skill.scope];
  const provider = PROVIDER_LABEL[skill.provider];
  return scope ? `${provider} · ${scope}` : provider;
}

/** Unique source tags across `skills`, in input order. */
export function skillSourceLabels(skills: Skill[]): string[] {
  return [...new Set(skills.map(skillSourceLabel))];
}

/** Copies of one skill whose content is identical. */
export interface SkillVersion {
  /** Every place this content was found; never empty. The first copy is
   *  the one Settings opens. */
  copies: Skill[];
}

export interface SkillEntry {
  name: string;
  /** One element unless same-named skills differ in content. */
  versions: SkillVersion[];
}

function hasContent(skill: Skill): boolean {
  return skill.readable !== false && skill.body.trim() !== "";
}

/** True when two same-named skills are one definition: the same file, or
 *  readable copies with identical frontmatter and body. Unreadable
 *  catalog entries only match by path, since their content is unknown. */
function sameDefinition(a: Skill, b: Skill): boolean {
  if (a.filePath !== "" && a.filePath === b.filePath) return true;
  return (
    hasContent(a) &&
    hasContent(b) &&
    a.body === b.body &&
    a.description === b.description &&
    JSON.stringify(a.rawFrontmatter) === JSON.stringify(b.rawFrontmatter)
  );
}

/**
 * Collapse skills into one entry per name, sorted case-insensitively.
 * Copies of the same definition share a version; a copy reached twice
 * through the same provider and scope (e.g. two symlinked roots of one
 * provider) is kept once.
 */
export function groupSkillsForSettings(skills: Skill[]): SkillEntry[] {
  const byName = new Map<string, SkillVersion[]>();
  for (const skill of skills) {
    let versions = byName.get(skill.name);
    if (!versions) {
      versions = [];
      byName.set(skill.name, versions);
    }
    const version = versions.find((candidate) =>
      candidate.copies.some((copy) => sameDefinition(copy, skill)),
    );
    if (!version) {
      versions.push({ copies: [skill] });
      continue;
    }
    const duplicate = version.copies.some(
      (copy) =>
        copy.id === skill.id &&
        copy.provider === skill.provider &&
        copy.scope === skill.scope,
    );
    if (!duplicate) version.copies.push(skill);
  }
  return [...byName.entries()]
    .map(([name, versions]) => ({ name, versions }))
    .sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
}

/**
 * Collapse same-named skills down to the one the composer will
 * actually use, preserving input order.
 *
 * A skill is addressed in the draft purely by name — typing
 * `/omarchy` cannot express "the Codex copy". So when the same name
 * is reachable through several roots (very common: `~/.claude/skills`,
 * `~/.codex/skills`, and `~/.agents/skills` are frequently symlinks to
 * one source, and a real directory may exist under `~/.codemux/skills`
 * as well) every copy inserts byte-identical text. Listing them all
 * offers a choice that does not exist, and — because rows are keyed by
 * skill id — copies reached through symlinks share an id, producing
 * duplicate React keys and colliding menu values.
 *
 * First-wins. The backend sorts provider → scope → name, so the
 * winner is the highest-priority provider at the narrowest scope,
 * which is the copy a user would expect to take precedence.
 *
 * Matching is case-SENSITIVE, deliberately. `parseSkillTokens` resolves
 * `/name` case-sensitively so that `/Plan` never silently expands to a
 * skill called `plan`, which makes names differing only in case two
 * genuinely distinct, separately addressable skills. Folding case here
 * would hide one of them from the menu while leaving it reachable by
 * typing — the exact popup/send-time drift this helper exists to
 * prevent. Real duplicates come from one source reached through
 * several roots, so their names match exactly anyway.
 *
 * Settings uses `groupSkillsForSettings` instead, which keeps every
 * install location visible and separates copies whose content differs.
 */
export function dedupeSkillsByName(skills: Skill[]): Skill[] {
  const seen = new Set<string>();
  const out: Skill[] = [];
  for (const skill of skills) {
    if (seen.has(skill.name)) continue;
    seen.add(skill.name);
    out.push(skill);
  }
  return out;
}
