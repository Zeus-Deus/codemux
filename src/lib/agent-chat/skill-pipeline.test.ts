// End-to-end test for the parse-then-prefix pipeline that AgentChatPane
// and DraftChatSurface stitch together at send time. Each layer
// (resolveSkillBodies + applyAllPrefixes) has its own focused unit
// tests; this file pins the *combined* contract so a refactor in one
// layer can't silently break the wire to the SDK.

import { describe, expect, it } from "vitest";

import type { Skill } from "@/tauri/commands";

import { applyAllPrefixes, ASK_WRAPPER } from "./mode-prefix";
import { resolveSkillBodies } from "./skill-tokens";
import { ULTRATHINK_PROMPT_PREFIX } from "./ultrathink";

function makeSkill(name: string, body: string): Skill {
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
];

/** Convenience: mirrors what AgentChatPane.handleSubmit and
 *  materializeAndSend do — parse text against the registry, then layer
 *  the bodies in via applyAllPrefixes. */
function pipeline(text: string, mode: Parameters<typeof applyAllPrefixes>[1], effort: string | null) {
  const bodies = resolveSkillBodies(text, REGISTRY);
  return applyAllPrefixes(text, mode, effort, bodies);
}

describe("skill pipeline (resolveSkillBodies → applyAllPrefixes)", () => {
  it("plain text with no skill tokens passes through unchanged", () => {
    expect(pipeline("hello world", "default", null)).toBe("hello world");
  });

  it("a single /skill token prepends its body framed by ---", () => {
    expect(pipeline("/omarchy", "default", null)).toBe(
      "OMARCHY BODY\n\n---\n\n/omarchy",
    );
  });

  it("mixed prose + token preserves the literal /skill in the user text", () => {
    expect(pipeline("hi test /omarchy", "default", null)).toBe(
      "OMARCHY BODY\n\n---\n\nhi test /omarchy",
    );
  });

  it("two skills stack with --- separator in source order", () => {
    expect(pipeline("/omarchy then /codemux-release", "default", null)).toBe(
      "OMARCHY BODY\n\n---\n\nRELEASE BODY\n\n---\n\n/omarchy then /codemux-release",
    );
  });

  it("unknown /token silently passes through with no body injected", () => {
    expect(pipeline("/notreal asks something", "default", null)).toBe(
      "/notreal asks something",
    );
  });

  it("composes ASK wrapper above the skill body", () => {
    expect(pipeline("hi /omarchy", "ask", null)).toBe(
      `${ASK_WRAPPER}\n\nOMARCHY BODY\n\n---\n\nhi /omarchy`,
    );
  });

  it("composes ultrathink + mode + skill in the locked top-to-bottom order", () => {
    expect(pipeline("hi /omarchy", "ask", "ultrathink")).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${ASK_WRAPPER}\n\nOMARCHY BODY\n\n---\n\nhi /omarchy`,
    );
  });

  it("dedupes a skill mentioned twice (registry-id collapse, body emitted once)", () => {
    expect(pipeline("/omarchy and /omarchy again", "default", null)).toBe(
      "OMARCHY BODY\n\n---\n\n/omarchy and /omarchy again",
    );
  });

  it("token at the very start of text resolves correctly", () => {
    expect(pipeline("/omarchy", "default", null)).toContain("OMARCHY BODY");
  });

  it("token must sit at a word boundary — 'text/omarchy' does not inject", () => {
    expect(pipeline("text/omarchy", "default", null)).toBe("text/omarchy");
  });
});
