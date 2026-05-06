import { describe, expect, it, vi } from "vitest";

import type { Skill } from "@/tauri/commands";

import {
  buildSkillCommands,
  formatScopeIndicator,
  formatSkillDescription,
} from "./skill-commands";

function makeSkill(overrides: Partial<Skill> = {}): Skill {
  return {
    id: "abc123",
    name: "demo",
    description: "Does demo things",
    provider: "claude",
    scope: "user",
    skillDir: "/skills/demo",
    filePath: "/skills/demo/SKILL.md",
    body: "",
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
    ...overrides,
  };
}

describe("buildSkillCommands", () => {
  it("maps each skill to a SlashCommandItem with expected shape", () => {
    const onInvoke = vi.fn();
    const items = buildSkillCommands({
      skills: [makeSkill({ id: "a", name: "alpha" }), makeSkill({ id: "b", name: "beta" })],
      onInvoke,
    });

    expect(items).toHaveLength(2);
    const [first] = items;
    expect(first.id).toBe("skill:a");
    expect(first.label).toBe("alpha");
    expect(first.command).toBe("/alpha");
    expect(first.group).toBe("SKILLS");
    expect(first.icon).toBeDefined();
  });

  it("preserves backend ordering", () => {
    const items = buildSkillCommands({
      skills: [
        makeSkill({ id: "1", name: "zeta" }),
        makeSkill({ id: "2", name: "alpha" }),
      ],
      onInvoke: () => {},
    });
    expect(items.map((i) => i.label)).toEqual(["zeta", "alpha"]);
  });

  it("onSelect forwards the original skill to onInvoke", () => {
    const onInvoke = vi.fn();
    const skill = makeSkill({ id: "xyz", name: "demo" });
    const [item] = buildSkillCommands({ skills: [skill], onInvoke });
    item.onSelect();
    expect(onInvoke).toHaveBeenCalledExactlyOnceWith(skill);
  });

  it("group is always SKILLS", () => {
    const items = buildSkillCommands({
      skills: [
        makeSkill({ provider: "claude" }),
        makeSkill({ provider: "codex" }),
        makeSkill({ provider: "codemux", scope: "project" }),
      ],
      onInvoke: () => {},
    });
    expect(items.every((i) => i.group === "SKILLS")).toBe(true);
  });
});

describe("formatSkillDescription", () => {
  it("appends scope indicator when description is present", () => {
    const out = formatSkillDescription(
      makeSkill({ description: "Releases a new version", provider: "claude", scope: "user" }),
    );
    expect(out).toBe("Releases a new version · claude · user");
  });

  it("returns scope only when description is missing", () => {
    const out = formatSkillDescription(
      makeSkill({ description: null, provider: "codex", scope: "user" }),
    );
    expect(out).toBe("codex · user");
  });

  it("returns scope only when description is empty string", () => {
    const out = formatSkillDescription(
      makeSkill({ description: "", provider: "claude", scope: "project" }),
    );
    expect(out).toBe("claude · project");
  });
});

describe("formatScopeIndicator", () => {
  it("plain provider · scope for user/project", () => {
    expect(formatScopeIndicator(makeSkill({ provider: "claude", scope: "user" }))).toBe(
      "claude · user",
    );
    expect(formatScopeIndicator(makeSkill({ provider: "codemux", scope: "project" }))).toBe(
      "codemux · project",
    );
  });

  it("plugin scope includes the plugin slug when available", () => {
    expect(
      formatScopeIndicator(
        makeSkill({ provider: "claude", scope: "plugin", pluginSlug: "frontend-design" }),
      ),
    ).toBe("claude · plugin/frontend-design");
  });

  it("plugin scope without slug falls back to bare scope", () => {
    expect(
      formatScopeIndicator(makeSkill({ provider: "claude", scope: "plugin", pluginSlug: null })),
    ).toBe("claude · plugin");
  });
});
