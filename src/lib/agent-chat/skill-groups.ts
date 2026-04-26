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
  return "Plugin";
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
