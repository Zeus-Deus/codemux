import { describe, expect, it } from "vitest";

import { buildCommandRegistry, parseLeadingCommand } from "./command-tokens";
import type { Skill } from "@/tauri/commands";

function makeSkill(name: string, overrides: Partial<Skill> = {}): Skill {
  return {
    id: `skill-${name}`,
    name,
    description: `${name} skill`,
    provider: "claude",
    scope: "user",
    skillDir: `/skills/${name}`,
    filePath: `/skills/${name}/SKILL.md`,
    body: `body of ${name}`,
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  } as Skill;
}

const GOAL = {
  name: "goal",
  description: "Set a standing goal for this thread",
  argumentHint: "<goal text>",
};

describe("buildCommandRegistry", () => {
  it("indexes provider commands and skills by lowercased name", () => {
    const registry = buildCommandRegistry([makeSkill("omarchy")], [GOAL]);
    expect(registry.get("goal")?.kind).toBe("provider");
    expect(registry.get("omarchy")?.kind).toBe("skill");
  });

  it("lets a skill win a name collision, matching the popup's precedence", () => {
    const registry = buildCommandRegistry(
      [makeSkill("review")],
      [{ name: "review", description: "Review a pull request" }],
    );
    expect(registry.get("review")?.kind).toBe("skill");
  });

  it("registers a collided skill under its provider-qualified token", () => {
    const skills = [
      makeSkill("review", { id: "a", provider: "claude", scope: "project" }),
      makeSkill("review", { id: "b", provider: "codex", scope: "user" }),
    ];
    const registry = buildCommandRegistry(skills, []);
    // Both definitions survive under distinct addresses; neither is
    // reachable as the bare `/review`, exactly as the popup inserts them.
    expect(registry.size).toBe(2);
    expect(registry.has("review")).toBe(false);
  });
});

describe("parseLeadingCommand", () => {
  const registry = buildCommandRegistry([makeSkill("omarchy")], [GOAL]);

  it("resolves a leading provider command with its arguments", () => {
    const match = parseLeadingCommand("/goal ship the release", registry);
    expect(match).toMatchObject({
      start: 0,
      end: 5,
      token: "/goal",
      name: "goal",
      kind: "provider",
      args: "ship the release",
      argumentHint: "<goal text>",
    });
  });

  it("reports empty args for a bare command so the hint can show", () => {
    expect(parseLeadingCommand("/goal", registry)?.args).toBe("");
    expect(parseLeadingCommand("/goal   ", registry)?.args).toBe("");
  });

  it("tolerates leading whitespace the runtime would trim anyway", () => {
    const match = parseLeadingCommand("  /goal ship it", registry);
    expect(match).toMatchObject({ start: 2, end: 7, name: "goal" });
  });

  it("resolves a leading skill token", () => {
    expect(parseLeadingCommand("/omarchy fix my bar", registry)).toMatchObject({
      name: "omarchy",
      kind: "skill",
      description: "omarchy skill",
    });
  });

  it("ignores an unregistered token — it is prose, not an invocation", () => {
    expect(parseLeadingCommand("/notacommand do it", registry)).toBeNull();
  });

  it("ignores an absolute path pasted at the start of a draft", () => {
    expect(
      parseLeadingCommand("/goal/subdir/file.md is the input", registry),
    ).toBeNull();
    expect(parseLeadingCommand("/home/zeus/notes.md", registry)).toBeNull();
  });

  it("ignores a command that does not lead the message", () => {
    // Providers only interpret a leading slash, so a mid-sentence
    // mention is literal text and must not be advertised as a run.
    expect(parseLeadingCommand("please run /goal later", registry)).toBeNull();
  });

  it("matches case-insensitively, the way the popup filters", () => {
    expect(parseLeadingCommand("/GOAL ship it", registry)?.name).toBe("GOAL");
  });
});
