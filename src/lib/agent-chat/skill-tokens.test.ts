import { describe, expect, it } from "vitest";

import type { Skill } from "@/tauri/commands";

import { buildSkillCommands } from "./skill-commands";
import {
  parseSkillTokens,
  rebaseSkillSelection,
  resolveSkillSelection,
  resolveSkillBodies,
  segmentForHighlight,
  skillsForProvider,
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
    const matches = parseSkillTokens(
      "line one\n/omarchy\nline three",
      REGISTRY,
    );
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
    expect(resolveSkillBodies("/omarchy then /codemux-release", REGISTRY)).toBe(
      "OMARCHY BODY\n\n---\n\nRELEASE BODY",
    );
  });

  it("dedupes by skill id when the same skill is mentioned twice", () => {
    expect(resolveSkillBodies("/omarchy /omarchy", REGISTRY)).toBe(
      "OMARCHY BODY",
    );
  });

  it("skips skills with empty / whitespace-only body", () => {
    const empty = makeSkill("blank", "   \n  ");
    expect(resolveSkillBodies("/blank", [empty])).toBeNull();
  });

  it("requires a qualified token when a name is duplicated", () => {
    const duplicated = [
      makeSkill("omarchy", "CLAUDE COPY"),
      {
        ...makeSkill("omarchy", "CODEX COPY"),
        id: "id-omarchy-codex",
        provider: "codex" as const,
      },
    ];
    expect(resolveSkillBodies("/omarchy", duplicated)).toBeNull();
    expect(resolveSkillBodies("/claude:user:omarchy", duplicated)).toBe(
      "CLAUDE COPY",
    );
    expect(resolveSkillBodies("/codex:user:omarchy", duplicated)).toBe(
      "CODEX COPY",
    );
  });
});

describe("popup / send-time agreement", () => {
  it("resolves every collision row to the exact offered skill", () => {
    const duplicated = [
      makeSkill("omarchy", "CLAUDE COPY"),
      {
        ...makeSkill("omarchy", "CODEX COPY"),
        id: "id-omarchy-codex",
        provider: "codex" as const,
      },
      {
        ...makeSkill("omarchy", "CODEMUX COPY"),
        id: "id-omarchy-codemux",
        provider: "codemux" as const,
      },
    ];

    const rows = buildSkillCommands({
      skills: duplicated,
      onInvoke: () => {},
    });
    expect(rows.map((row) => row.command)).toEqual([
      "/claude:user:omarchy",
      "/codex:user:omarchy",
      "/codemux:user:omarchy",
    ]);
    for (const row of rows) {
      const [resolved] = parseSkillTokens(row.command, duplicated);
      expect(resolved.skill.provider).toBe(row.command.split(":")[0].slice(1));
    }
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

describe("resolveSkillSelection", () => {
  it("carries exact ids and strips only recognized skill tokens", () => {
    expect(
      resolveSkillSelection(
        "/omarchy please keep /compact and /not-a-skill",
        REGISTRY,
      ),
    ).toEqual({
      skillIds: ["id-omarchy"],
      text: "please keep /compact and /not-a-skill",
    });
  });

  it("preserves unrelated indentation and repeated spaces byte-for-byte", () => {
    expect(
      resolveSkillSelection(
        "  keep  spacing\n    /omarchy next  value",
        REGISTRY,
      ),
    ).toEqual({
      skillIds: ["id-omarchy"],
      text: "  keep  spacing\n    next  value",
    });
  });

  it("preserves collision identity through qualified tokens", () => {
    const duplicated = [
      makeSkill("deploy", "CLAUDE"),
      {
        ...makeSkill("deploy", "CODEX"),
        id: "id-deploy-codex",
        provider: "codex" as const,
      },
    ];
    expect(
      resolveSkillSelection("/codex:user:deploy now", duplicated),
    ).toEqual({ skillIds: ["id-deploy-codex"], text: "now" });
  });

  it("uses provider availability consistently when addressing collisions", () => {
    const available = {
      ...makeSkill("deploy", "CLAUDE"),
      projections: [
        {
          targetProvider: "claude" as const,
          availability: "native" as const,
          compatibility: "compatible" as const,
          reasons: [],
          invocation: "native-command" as const,
        },
      ],
    };
    const unavailable = {
      ...makeSkill("deploy", "CODEX"),
      id: "id-deploy-codex",
      provider: "codex" as const,
      projections: [
        {
          targetProvider: "claude" as const,
          availability: "unavailable" as const,
          compatibility: "compatible" as const,
          reasons: [],
          invocation: "none" as const,
        },
      ],
    };
    const registry = skillsForProvider([available, unavailable], "claude");

    expect(resolveSkillSelection("/deploy now", registry)).toEqual({
      skillIds: [available.id],
      text: "now",
    });
  });

  it("extends collision suffixes until every textual address is unique", () => {
    const first = { ...makeSkill("deploy"), id: "abcdef1aaa" };
    const second = { ...makeSkill("deploy"), id: "abcdef2bbb" };
    const registry = [first, second];

    expect(resolveSkillSelection("/claude:user:deploy:abcdef1 run", registry)).toEqual({
      skillIds: [first.id],
      text: "run",
    });
    expect(resolveSkillSelection("/claude:user:deploy:abcdef2 run", registry)).toEqual({
      skillIds: [second.id],
      text: "run",
    });
  });

  it("rebases an exact project definition into a deferred worktree", () => {
    const source = {
      ...makeSkill("review"),
      id: "source-id",
      scope: "project" as const,
      filePath: "/repo/.claude/skills/review/SKILL.md",
    };
    const target = {
      ...source,
      id: "target-id",
      filePath: "/repo-review/.claude/skills/review/SKILL.md",
    };

    expect(
      rebaseSkillSelection(
        { skillIds: [source.id], text: "check it" },
        [source],
        "/repo",
        "/repo-review",
        [target],
      ),
    ).toEqual({ skillIds: [target.id], text: "check it" });
  });
});

describe("skillsForProvider", () => {
  function withProjection(
    skill: Skill,
    targetProvider: "claude" | "codex" | "opencode",
    availability: "native" | "explicit-portable" | "native-only" | "unavailable",
  ): Skill {
    return {
      ...skill,
      projections: [
        {
          targetProvider,
          availability,
          compatibility: "compatible",
          reasons: [],
          invocation:
            availability === "unavailable" || availability === "native-only"
              ? "none"
              : "prompt-prefix",
        },
      ],
    };
  }

  it("uses the Codex-portable projection for Grok", () => {
    const portable = withProjection(
      makeSkill("portable"),
      "codex",
      "explicit-portable",
    );
    const unavailable = withProjection(
      makeSkill("unavailable"),
      "codex",
      "unavailable",
    );

    expect(skillsForProvider([portable, unavailable], "grok")).toEqual([
      portable,
    ]);
  });

  it("fails closed when a projection-aware skill omits Grok's Codex target", () => {
    const claudeOnly = withProjection(
      makeSkill("claude-only"),
      "claude",
      "native",
    );

    expect(skillsForProvider([claudeOnly], "grok")).toEqual([]);
  });
});
