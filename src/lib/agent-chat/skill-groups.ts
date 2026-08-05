// Group + sort skills for the Settings → Skills section. Backend
// already returns skills in a stable order (provider → scope → name);
// the Settings UI re-buckets them into human-readable headings the
// user sees in the sidebar nav, with one bucket per provider/scope
// combination plus a single "Plugin" bucket that collapses Claude's
// marketplace + external_plugins skills regardless of plugin slug.

import type { Skill, SkillProvider, SkillScope } from "@/tauri/commands";

const PROVIDER_LABEL: Record<SkillProvider, string> = {
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
  codemux: "Codemux",
};

/**
 * Display heading for a skill's group in the Settings UI. Plugin-scope
 * skills collapse into a single "Plugin" group regardless of provider
 * (in practice all plugins are Claude today; the schema permits
 * provider variance and we keep it future-proof by hiding it here).
 */
export function groupHeadingFor(skill: Skill): string {
  if (skill.scope === "plugin") return "Plugin";
  return `${capitalizeScope(skill.scope)} · ${PROVIDER_LABEL[skill.provider]}`;
}

function capitalizeScope(scope: SkillScope): string {
  if (scope === "user") return "User";
  if (scope === "project") return "Project";
  if (scope === "plugin") return "Plugin";
  return scope.charAt(0).toUpperCase() + scope.slice(1);
}

/**
 * Stable ordering for groups. Anything not in the list (future
 * provider scopes) sinks to the end alphabetically.
 */
export const GROUP_ORDER: ReadonlyArray<string> = [
  "User · Claude",
  "User · Codex",
  "User · OpenCode",
  "User · Codemux",
  "Project · Claude",
  "Project · Codex",
  "Project · OpenCode",
  "Project · Codemux",
  "Plugin",
];

export interface SkillGroup {
  heading: string;
  skills: Skill[];
}

/**
 * Find skills that share a name across different scopes/providers.
 * Returns a Map keyed by name → skills (always ≥2 entries per key).
 * Skills with unique names are excluded entirely.
 *
 * Used by the Settings UI to surface naming clashes at the top of
 * the page so users can disambiguate explicitly. The slash popup
 * itself disambiguates inline via the description suffix (`provider
 * · scope`) and never silently picks one over the other.
 */
export function detectConflicts(skills: Skill[]): Map<string, Skill[]> {
  const byName = new Map<string, Skill[]>();
  for (const skill of skills) {
    const list = byName.get(skill.name);
    if (list) list.push(skill);
    else byName.set(skill.name, [skill]);
  }
  for (const name of [...byName.keys()]) {
    if ((byName.get(name) ?? []).length <= 1) byName.delete(name);
  }
  return byName;
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
 * This is deliberately NOT applied to the Settings list: seeing every
 * install location is the point there, and `detectConflicts` above
 * surfaces exactly these clashes so they stay discoverable.
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

/**
 * Bucket skills by their group heading, returning groups in
 * `GROUP_ORDER`. Within each group skills are sorted alphabetically
 * by name (case-insensitive). Empty groups are dropped.
 */
export function groupSkillsByScope(skills: Skill[]): SkillGroup[] {
  const buckets = new Map<string, Skill[]>();
  for (const skill of skills) {
    const heading = groupHeadingFor(skill);
    const list = buckets.get(heading);
    if (list) list.push(skill);
    else buckets.set(heading, [skill]);
  }

  const out: SkillGroup[] = [];
  // First pass: emit groups in `GROUP_ORDER` if present.
  for (const heading of GROUP_ORDER) {
    const skillsForHeading = buckets.get(heading);
    if (skillsForHeading && skillsForHeading.length > 0) {
      out.push({
        heading,
        skills: [...skillsForHeading].sort((a, b) =>
          a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
        ),
      });
      buckets.delete(heading);
    }
  }
  // Second pass: any unknown headings (future schema) sink to the end
  // sorted alphabetically by heading so users see a stable list.
  const remaining = [...buckets.entries()].sort(([a], [b]) =>
    a.localeCompare(b),
  );
  for (const [heading, skillsForHeading] of remaining) {
    out.push({
      heading,
      skills: [...skillsForHeading].sort((a, b) =>
        a.name.toLowerCase().localeCompare(b.name.toLowerCase()),
      ),
    });
  }
  return out;
}
