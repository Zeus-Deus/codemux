import { describe, expect, it } from "vitest";

import type { Skill, SkillProvider, SkillScope } from "@/tauri/commands";

import {
  GROUP_ORDER,
  dedupeSkillsByName,
  detectConflicts,
  groupHeadingFor,
  groupSkillsByScope,
} from "./skill-groups";

function makeSkill(
  name: string,
  provider: SkillProvider,
  scope: SkillScope,
  pluginSlug: string | null = null,
): Skill {
  return {
    id: `id-${name}`,
    name,
    description: null,
    provider,
    scope,
    skillDir: `/skills/${name}`,
    filePath: `/skills/${name}/SKILL.md`,
    body: "",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug,
  };
}

describe("groupHeadingFor", () => {
  it("emits 'Scope · Provider' for non-plugin skills", () => {
    expect(groupHeadingFor(makeSkill("a", "claude", "user"))).toBe(
      "User · Claude",
    );
    expect(groupHeadingFor(makeSkill("a", "codex", "user"))).toBe(
      "User · Codex",
    );
    expect(groupHeadingFor(makeSkill("a", "claude", "project"))).toBe(
      "Project · Claude",
    );
    expect(groupHeadingFor(makeSkill("a", "codemux", "user"))).toBe(
      "User · Codemux",
    );
    expect(groupHeadingFor(makeSkill("a", "opencode", "project"))).toBe(
      "Project · OpenCode",
    );
  });

  it("collapses every plugin scope into a single 'Plugin' bucket", () => {
    expect(
      groupHeadingFor(makeSkill("a", "claude", "plugin", "frontend-design")),
    ).toBe("Plugin");
  });
});

describe("groupSkillsByScope", () => {
  it("returns groups in GROUP_ORDER, dropping empty buckets", () => {
    const groups = groupSkillsByScope([
      makeSkill("plugin-skill", "claude", "plugin", "frontend-design"),
      makeSkill("user-codex", "codex", "user"),
      makeSkill("user-claude", "claude", "user"),
    ]);
    expect(groups.map((g) => g.heading)).toEqual([
      "User · Claude",
      "User · Codex",
      "Plugin",
    ]);
  });

  it("sorts skills within a group alphabetically (case-insensitive)", () => {
    const groups = groupSkillsByScope([
      makeSkill("Zeta", "claude", "user"),
      makeSkill("alpha", "claude", "user"),
      makeSkill("beta", "claude", "user"),
    ]);
    expect(groups[0].skills.map((s) => s.name)).toEqual([
      "alpha",
      "beta",
      "Zeta",
    ]);
  });

  it("returns an empty array when no skills are passed", () => {
    expect(groupSkillsByScope([])).toEqual([]);
  });

  it("emits all 9 known groups in the locked order when skills span them", () => {
    const skills: Skill[] = [
      makeSkill("a", "claude", "user"),
      makeSkill("b", "codex", "user"),
      makeSkill("c", "opencode", "user"),
      makeSkill("d", "codemux", "user"),
      makeSkill("e", "claude", "project"),
      makeSkill("f", "codex", "project"),
      makeSkill("g", "opencode", "project"),
      makeSkill("h", "codemux", "project"),
      makeSkill("i", "claude", "plugin", "p"),
    ];
    const groups = groupSkillsByScope(skills);
    expect(groups.map((g) => g.heading)).toEqual([...GROUP_ORDER]);
  });

  it("detectConflicts returns nothing when every name is unique", () => {
    const conflicts = detectConflicts([
      makeSkill("alpha", "claude", "user"),
      makeSkill("beta", "claude", "user"),
      makeSkill("gamma", "codex", "user"),
    ]);
    expect(conflicts.size).toBe(0);
  });

  it("detectConflicts surfaces same-name skills across scopes", () => {
    const userRelease = makeSkill("release", "claude", "user");
    const projectRelease = makeSkill("release", "claude", "project");
    const conflicts = detectConflicts([
      userRelease,
      projectRelease,
      makeSkill("other", "claude", "user"),
    ]);
    expect(conflicts.size).toBe(1);
    expect(conflicts.get("release")).toHaveLength(2);
  });

  it("detectConflicts surfaces same-name skills across providers", () => {
    const conflicts = detectConflicts([
      makeSkill("foo", "claude", "user"),
      makeSkill("foo", "codex", "user"),
    ]);
    expect(conflicts.size).toBe(1);
    expect(conflicts.get("foo")?.map((s) => s.provider).sort()).toEqual([
      "claude",
      "codex",
    ]);
  });

  it("detectConflicts ignores skills that have only one entry per name", () => {
    const conflicts = detectConflicts([
      makeSkill("solo", "claude", "user"),
    ]);
    expect(conflicts.size).toBe(0);
  });

  it("dedupeSkillsByName keeps the first copy of a name, dropping the rest", () => {
    // The real-world shape: one source symlinked into several provider
    // roots, so the same name arrives once per root.
    const out = dedupeSkillsByName([
      makeSkill("omarchy", "claude", "user"),
      makeSkill("omarchy", "codex", "user"),
      makeSkill("omarchy", "codemux", "user"),
      makeSkill("solo", "claude", "user"),
    ]);
    expect(out).toHaveLength(2);
    expect(out[0].provider).toBe("claude");
    expect(out[1].name).toBe("solo");
  });

  it("dedupeSkillsByName keeps names that differ only in case", () => {
    // `parseSkillTokens` resolves case-sensitively, so these are two
    // separately addressable skills. Folding them would hide one from
    // the menu while leaving it reachable by typing.
    const out = dedupeSkillsByName([
      makeSkill("Deploy", "claude", "user"),
      makeSkill("deploy", "codex", "user"),
    ]);
    expect(out).toHaveLength(2);
  });

  it("dedupeSkillsByName leaves distinct names untouched and preserves order", () => {
    const out = dedupeSkillsByName([
      makeSkill("b", "claude", "user"),
      makeSkill("a", "codex", "user"),
    ]);
    expect(out.map((s) => s.name)).toEqual(["b", "a"]);
  });

  it("dedupeSkillsByName does not mutate its input", () => {
    const input = [
      makeSkill("dup", "claude", "user"),
      makeSkill("dup", "codex", "user"),
    ];
    dedupeSkillsByName(input);
    expect(input).toHaveLength(2);
  });

  it("detectConflicts still reports duplicates that the popup collapses", () => {
    // Settings must keep showing every install location even though
    // the composer only ever offers one — the two behaviours are
    // intentionally different views of the same data.
    const skills = [
      makeSkill("omarchy", "claude", "user"),
      makeSkill("omarchy", "codex", "user"),
    ];
    expect(dedupeSkillsByName(skills)).toHaveLength(1);
    expect(detectConflicts(skills).get("omarchy")).toHaveLength(2);
  });

  it("produces independent arrays per group (mutating one doesn't bleed)", () => {
    const groups = groupSkillsByScope([
      makeSkill("a", "claude", "user"),
      makeSkill("b", "claude", "user"),
    ]);
    groups[0].skills.pop();
    const groupsAgain = groupSkillsByScope([
      makeSkill("a", "claude", "user"),
      makeSkill("b", "claude", "user"),
    ]);
    expect(groupsAgain[0].skills).toHaveLength(2);
  });
});
