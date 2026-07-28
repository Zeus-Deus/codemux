import { describe, expect, it } from "vitest";

import type { Skill } from "@/tauri/commands";

import { buildSkillCommands } from "./skill-commands";
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

  it("resolves a duplicated name to the FIRST copy, matching the popup", () => {
    // Regression: the name lookup used to be last-wins while the popup
    // was first-wins, so picking the top `/omarchy` row injected the
    // last copy's body.
    const duplicated = [
      makeSkill("omarchy", "CLAUDE COPY"),
      { ...makeSkill("omarchy", "CODEX COPY"), id: "id-omarchy-codex" },
    ];
    expect(resolveSkillBodies("/omarchy", duplicated)).toBe("CLAUDE COPY");
  });
});

describe("popup / send-time agreement", () => {
  it("injects the body of exactly the skill the popup offered", () => {
    // The two surfaces must not drift: whatever single row the menu
    // shows for a duplicated name is the body the model receives.
    const duplicated = [
      makeSkill("omarchy", "CLAUDE COPY"),
      { ...makeSkill("omarchy", "CODEX COPY"), id: "id-omarchy-codex" },
      { ...makeSkill("omarchy", "CODEMUX COPY"), id: "id-omarchy-codemux" },
    ];

    const rows = buildSkillCommands({
      skills: duplicated,
      onInvoke: () => {},
    });
    const omarchyRows = rows.filter((r) => r.command === "/omarchy");
    expect(omarchyRows).toHaveLength(1);

    const offeredId = omarchyRows[0].id.replace(/^skill:/, "");
    const [resolved] = parseSkillTokens("/omarchy", duplicated);
    expect(resolved.skill.id).toBe(offeredId);
    expect(resolveSkillBodies("/omarchy", duplicated)).toBe("CLAUDE COPY");
  });

  it("keeps case-variant names reachable from both surfaces", () => {
    // The dedupe must not fold case, because the resolver does not.
    const variants = [
      { ...makeSkill("Deploy", "UPPER BODY"), id: "id-Deploy" },
      { ...makeSkill("deploy", "lower body"), id: "id-deploy" },
    ];
    const rows = buildSkillCommands({ skills: variants, onInvoke: () => {} });
    expect(rows).toHaveLength(2);
    expect(resolveSkillBodies("/Deploy", variants)).toBe("UPPER BODY");
    expect(resolveSkillBodies("/deploy", variants)).toBe("lower body");
  });

  it("gives every popup row a unique id so menu keys cannot collide", () => {
    // Symlinked copies share a canonical path, hence the same skill id
    // — duplicate React keys / menu values before the dedupe.
    const symlinked = [
      makeSkill("omarchy", "BODY"),
      makeSkill("omarchy", "BODY"),
    ];
    expect(symlinked[0].id).toBe(symlinked[1].id);

    const ids = buildSkillCommands({
      skills: symlinked,
      onInvoke: () => {},
    }).map((r) => r.id);
    expect(new Set(ids).size).toBe(ids.length);
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
