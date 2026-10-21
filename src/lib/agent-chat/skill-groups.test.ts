import { describe, expect, it } from "vitest";

import type { Skill, SkillProvider, SkillScope } from "@/tauri/commands";

import {
  dedupeSkillsByName,
  groupSkillsForSettings,
  skillSourceLabel,
  skillSourceLabels,
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

/** A copy of `name` stored at its own path with the given content. */
function copyAt(
  name: string,
  provider: SkillProvider,
  path: string,
  body: string,
  scope: SkillScope = "user",
): Skill {
  return {
    ...makeSkill(name, provider, scope),
    id: `id-${path}`,
    filePath: `${path}/SKILL.md`,
    skillDir: path,
    body,
  };
}

describe("skillSourceLabel", () => {
  it("names the provider alone for user-wide and configured skills", () => {
    expect(skillSourceLabel(makeSkill("a", "claude", "user"))).toBe("Claude");
    expect(skillSourceLabel(makeSkill("a", "opencode", "configured"))).toBe(
      "OpenCode",
    );
  });

  it("adds narrower scopes and collapses plugins", () => {
    expect(skillSourceLabel(makeSkill("a", "codex", "project"))).toBe(
      "Codex · Project",
    );
    expect(
      skillSourceLabel(makeSkill("a", "claude", "plugin", "frontend-design")),
    ).toBe("Plugin");
  });

  it("lists each source once, in input order", () => {
    expect(
      skillSourceLabels([
        makeSkill("a", "codex", "user"),
        makeSkill("b", "claude", "user"),
        makeSkill("c", "codex", "user"),
      ]),
    ).toEqual(["Codex", "Claude"]);
  });
});

describe("groupSkillsForSettings", () => {
  it("merges one file reached from several providers into one version", () => {
    // makeSkill gives same-named skills the same canonical path, the
    // shape a symlinked source produces.
    const entries = groupSkillsForSettings([
      makeSkill("omarchy", "claude", "user"),
      makeSkill("omarchy", "codex", "user"),
      makeSkill("omarchy", "codemux", "user"),
    ]);
    expect(entries).toHaveLength(1);
    expect(entries[0].versions).toHaveLength(1);
    expect(entries[0].versions[0].copies.map((s) => s.provider)).toEqual([
      "claude",
      "codex",
      "codemux",
    ]);
  });

  it("merges separate copies whose content is identical", () => {
    const entries = groupSkillsForSettings([
      copyAt("file-pr", "claude", "/fleet/file-pr", "Same body."),
      copyAt("file-pr", "codemux", "/codemux/file-pr", "Same body."),
    ]);
    expect(entries[0].versions).toHaveLength(1);
    expect(entries[0].versions[0].copies).toHaveLength(2);
  });

  it("splits copies whose content differs into versions", () => {
    const entries = groupSkillsForSettings([
      copyAt("omarchy", "claude", "/system/omarchy", "New body."),
      copyAt("omarchy", "codex", "/system/omarchy", "New body."),
      copyAt("omarchy", "codemux", "/codemux/omarchy", "Old body."),
    ]);
    expect(entries[0].versions.map((v) => v.copies.length)).toEqual([2, 1]);
    expect(entries[0].versions[1].copies[0].provider).toBe("codemux");
  });

  it("never merges unreadable copies by their empty content", () => {
    const entries = groupSkillsForSettings([
      { ...copyAt("hidden", "codex", "/a/hidden", ""), readable: false },
      { ...copyAt("hidden", "codex", "/b/hidden", ""), readable: false },
    ]);
    expect(entries[0].versions).toHaveLength(2);
  });

  it("keeps a file reached twice through the same provider once", () => {
    const entries = groupSkillsForSettings([
      makeSkill("diagnose-crash", "codex", "user"),
      makeSkill("diagnose-crash", "codex", "user"),
      makeSkill("diagnose-crash", "claude", "user"),
    ]);
    expect(
      entries[0].versions[0].copies.map((s) => s.provider),
    ).toEqual(["codex", "claude"]);
  });

  it("sorts entries by name, case-insensitively", () => {
    const entries = groupSkillsForSettings([
      makeSkill("Zeta", "claude", "user"),
      makeSkill("alpha", "codex", "user"),
      makeSkill("beta", "claude", "plugin", "p"),
    ]);
    expect(entries.map((e) => e.name)).toEqual(["alpha", "beta", "Zeta"]);
  });

  it("returns an empty array when no skills are passed", () => {
    expect(groupSkillsForSettings([])).toEqual([]);
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

  it("Settings keeps every install location that the popup collapses", () => {
    // The composer only ever offers one copy; Settings still shows
    // where each copy lives.
    const skills = [
      makeSkill("omarchy", "claude", "user"),
      makeSkill("omarchy", "codex", "user"),
    ];
    expect(dedupeSkillsByName(skills)).toHaveLength(1);
    expect(groupSkillsForSettings(skills)[0].versions[0].copies).toHaveLength(2);
  });
});
