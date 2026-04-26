import { describe, expect, it } from "vitest";

import type { Skill } from "@/tauri/commands";

import {
  parseSkillTokens,
  resolveSkillBodies,
  segmentForHighlight,
} from "./skill-tokens";

function makeSkill(name: string, body = `Body for ${name}.`): Skill {
  return {
    id: `id-${name}`,
    name,
    description: null,
    provider: "claude",
    scope: "user",
    skillDir: `/skills/${name}`,
    filePath: `/skills/${name}/SKILL.md`,
    body,
    rawFrontmatter: {},
    bundledFiles: [],
    compatibility: "compatible",
    compatibilitySignals: [],
    symlinked: false,
    pluginSlug: null,
  };
}

const REGISTRY = [
  makeSkill("omarchy", "OMARCHY BODY"),
  makeSkill("codemux-release", "RELEASE BODY"),
  makeSkill("codemux-ui", "UI BODY"),
];

describe("parseSkillTokens", () => {
  it("matches a single token at start of text", () => {
    const matches = parseSkillTokens("/omarchy", REGISTRY);
    expect(matches).toHaveLength(1);
    expect(matches[0]).toMatchObject({
      start: 0,
      end: 8,
      token: "/omarchy",
      name: "omarchy",
    });
  });

  it("matches a token after whitespace", () => {
    const matches = parseSkillTokens("hi test /omarchy", REGISTRY);
    expect(matches).toHaveLength(1);
    expect(matches[0].token).toBe("/omarchy");
    expect(matches[0].start).toBe(8);
  });

  it("does not match a slash inside a word", () => {
    expect(parseSkillTokens("text/omarchy", REGISTRY)).toEqual([]);
    expect(parseSkillTokens("a/b/omarchy", REGISTRY)).toEqual([]);
  });

  it("treats trailing punctuation as a boundary, not part of the name", () => {
    const matches = parseSkillTokens("Run /omarchy.", REGISTRY);
    expect(matches).toHaveLength(1);
    expect(matches[0].token).toBe("/omarchy");
  });

  it("supports hyphenated skill names", () => {
    const matches = parseSkillTokens("/codemux-release", REGISTRY);
    expect(matches).toHaveLength(1);
    expect(matches[0].name).toBe("codemux-release");
  });

  it("does not match an unregistered name (silent miss, not error)", () => {
    expect(parseSkillTokens("/unknown", REGISTRY)).toEqual([]);
  });

  it("matches multiple tokens in order", () => {
    const matches = parseSkillTokens(
      "first /omarchy then /codemux-release end",
      REGISTRY,
    );
    expect(matches.map((m) => m.name)).toEqual(["omarchy", "codemux-release"]);
  });

  it("matches the same token twice when mentioned twice (no dedupe at parse layer)", () => {
    const matches = parseSkillTokens("/omarchy and /omarchy again", REGISTRY);
    expect(matches).toHaveLength(2);
  });

  it("does not match across newlines incorrectly", () => {
    const matches = parseSkillTokens("line one\n/omarchy\nline three", REGISTRY);
    expect(matches).toHaveLength(1);
    expect(matches[0].name).toBe("omarchy");
  });

  it("returns empty for empty text or empty registry", () => {
    expect(parseSkillTokens("", REGISTRY)).toEqual([]);
    expect(parseSkillTokens("/omarchy", [])).toEqual([]);
  });

  it("is case-sensitive (avoids surprising substitutions)", () => {
    expect(parseSkillTokens("/Omarchy", REGISTRY)).toEqual([]);
  });

  it("is stateless across calls (regex.lastIndex is reset)", () => {
    parseSkillTokens("/omarchy", REGISTRY);
    const again = parseSkillTokens("/omarchy", REGISTRY);
    expect(again).toHaveLength(1);
    expect(again[0].start).toBe(0);
  });
});

describe("resolveSkillBodies", () => {
  it("returns null when no skills match", () => {
    expect(resolveSkillBodies("plain text", REGISTRY)).toBeNull();
    expect(resolveSkillBodies("", REGISTRY)).toBeNull();
    expect(resolveSkillBodies("/notreal", REGISTRY)).toBeNull();
  });

  it("returns the trimmed body for a single match", () => {
    expect(resolveSkillBodies("/omarchy", REGISTRY)).toBe("OMARCHY BODY");
  });

  it("concatenates multiple bodies with --- separators in source order", () => {
    expect(
      resolveSkillBodies("/omarchy then /codemux-release", REGISTRY),
    ).toBe("OMARCHY BODY\n\n---\n\nRELEASE BODY");
  });

  it("dedupes by skill id when the same skill is mentioned twice", () => {
    expect(resolveSkillBodies("/omarchy /omarchy", REGISTRY)).toBe("OMARCHY BODY");
  });

  it("skips skills with empty / whitespace-only body", () => {
    const empty = makeSkill("blank", "   \n  ");
    expect(resolveSkillBodies("/blank", [empty])).toBeNull();
  });
});

describe("segmentForHighlight", () => {
  it("returns a single plain segment when no tokens match", () => {
    expect(segmentForHighlight("plain text", REGISTRY)).toEqual([
      { kind: "plain", text: "plain text" },
    ]);
  });

  it("returns an empty array for empty text", () => {
    expect(segmentForHighlight("", REGISTRY)).toEqual([]);
  });

  it("interleaves plain and skill segments preserving original characters", () => {
    expect(segmentForHighlight("hi /omarchy bye", REGISTRY)).toEqual([
      { kind: "plain", text: "hi " },
      { kind: "skill", text: "/omarchy", name: "omarchy" },
      { kind: "plain", text: " bye" },
    ]);
  });

  it("emits a leading skill segment when text starts with a token", () => {
    expect(segmentForHighlight("/omarchy", REGISTRY)).toEqual([
      { kind: "skill", text: "/omarchy", name: "omarchy" },
    ]);
  });

  it("emits a trailing skill segment when text ends with a token", () => {
    expect(segmentForHighlight("text /omarchy", REGISTRY)).toEqual([
      { kind: "plain", text: "text " },
      { kind: "skill", text: "/omarchy", name: "omarchy" },
    ]);
  });

  it("preserves whitespace runs and newlines verbatim", () => {
    expect(segmentForHighlight("  /omarchy\n\nmore", REGISTRY)).toEqual([
      { kind: "plain", text: "  " },
      { kind: "skill", text: "/omarchy", name: "omarchy" },
      { kind: "plain", text: "\n\nmore" },
    ]);
  });

  it("emits a plain segment for unmatched slash tokens", () => {
    expect(segmentForHighlight("/notreal", REGISTRY)).toEqual([
      { kind: "plain", text: "/notreal" },
    ]);
  });
});
