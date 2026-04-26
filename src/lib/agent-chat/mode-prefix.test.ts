import { describe, it, expect } from "vitest";

import {
  applyAllPrefixes,
  applyModePrefix,
  applySkillPrefix,
  ASK_WRAPPER,
  DEBUG_WRAPPER,
} from "./mode-prefix";
import { ULTRATHINK_PROMPT_PREFIX } from "./ultrathink";

const SKILL_BODY = "You are running the codemux-release skill. First, run gh release create.";

describe("applyModePrefix", () => {
  it("prepends ASK_WRAPPER for ask mode", () => {
    expect(applyModePrefix("what does X do?", "ask")).toBe(
      `${ASK_WRAPPER}\n\nwhat does X do?`,
    );
  });

  it("passes through unchanged for default mode", () => {
    expect(applyModePrefix("hello", "default")).toBe("hello");
  });

  it("passes through unchanged for plan mode (Plan uses SDK enforcement, not a wrapper)", () => {
    expect(applyModePrefix("draft a plan", "plan")).toBe("draft a plan");
  });

  it("prepends DEBUG_WRAPPER for debug mode", () => {
    expect(applyModePrefix("crash on submit", "debug")).toBe(
      `${DEBUG_WRAPPER}\n\ncrash on submit`,
    );
  });

  it("is idempotent — re-applying ask wrapper does not double-wrap", () => {
    const once = applyModePrefix("what does X do?", "ask");
    const twice = applyModePrefix(once, "ask");
    expect(twice).toBe(once);
  });

  it("is idempotent — re-applying debug wrapper does not double-wrap", () => {
    const once = applyModePrefix("crash on submit", "debug");
    const twice = applyModePrefix(once, "debug");
    expect(twice).toBe(once);
  });

  it("trims whitespace around the body before wrapping", () => {
    expect(applyModePrefix("   investigate  ", "ask")).toBe(
      `${ASK_WRAPPER}\n\ninvestigate`,
    );
    expect(applyModePrefix("   investigate  ", "debug")).toBe(
      `${DEBUG_WRAPPER}\n\ninvestigate`,
    );
  });

  it("returns empty for empty / whitespace-only inputs", () => {
    expect(applyModePrefix("", "ask")).toBe("");
    expect(applyModePrefix("   ", "ask")).toBe("");
    expect(applyModePrefix("", "debug")).toBe("");
    expect(applyModePrefix("   ", "debug")).toBe("");
  });
});

describe("applyAllPrefixes", () => {
  it("composes ask wrapper + ultrathink in the locked order: ultrathink → mode → user text", () => {
    const result = applyAllPrefixes("what does X do?", "ask", "ultrathink");
    expect(result).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${ASK_WRAPPER}\n\nwhat does X do?`,
    );
  });

  it("ask alone wraps without ultrathink prefix", () => {
    expect(applyAllPrefixes("what does X do?", "ask", null)).toBe(
      `${ASK_WRAPPER}\n\nwhat does X do?`,
    );
  });

  it("ultrathink alone (default mode) only adds the ultrathink prefix", () => {
    expect(applyAllPrefixes("investigate", "default", "ultrathink")).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}investigate`,
    );
  });

  it("default mode + non-ultrathink effort passes text through unchanged", () => {
    expect(applyAllPrefixes("hello", "default", "high")).toBe("hello");
    expect(applyAllPrefixes("hello", "default", null)).toBe("hello");
  });

  it("plan mode does not contribute a wrapper (SDK enforcement only)", () => {
    expect(applyAllPrefixes("design X", "plan", null)).toBe("design X");
    expect(applyAllPrefixes("design X", "plan", "ultrathink")).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}design X`,
    );
  });

  it("composes debug wrapper + ultrathink in the locked order: ultrathink → mode → user text", () => {
    const result = applyAllPrefixes("crash on submit", "debug", "ultrathink");
    expect(result).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${DEBUG_WRAPPER}\n\ncrash on submit`,
    );
  });

  it("debug alone wraps without ultrathink prefix", () => {
    expect(applyAllPrefixes("crash on submit", "debug", null)).toBe(
      `${DEBUG_WRAPPER}\n\ncrash on submit`,
    );
  });

  it("is idempotent under repeated application", () => {
    const once = applyAllPrefixes("ask question", "ask", "ultrathink");
    const twice = applyAllPrefixes(once, "ask", "ultrathink");
    expect(twice).toBe(once);
  });

  it("is idempotent under repeated application for debug + ultrathink", () => {
    const once = applyAllPrefixes("crash on submit", "debug", "ultrathink");
    const twice = applyAllPrefixes(once, "debug", "ultrathink");
    expect(twice).toBe(once);
  });

  it("returns empty for empty input regardless of mode / effort", () => {
    expect(applyAllPrefixes("", "ask", "ultrathink")).toBe("");
    expect(applyAllPrefixes("   ", "default", "ultrathink")).toBe("");
  });
});

describe("applySkillPrefix", () => {
  it("prepends a trimmed skill body framed by ---", () => {
    expect(applySkillPrefix("do the thing", `${SKILL_BODY}\n\n`)).toBe(
      `${SKILL_BODY}\n\n---\n\ndo the thing`,
    );
  });

  it("returns text unchanged when stagedSkillBody is null/undefined", () => {
    expect(applySkillPrefix("do the thing", null)).toBe("do the thing");
    expect(applySkillPrefix("do the thing", undefined)).toBe("do the thing");
  });

  it("returns text unchanged when stagedSkillBody is empty / whitespace", () => {
    expect(applySkillPrefix("do the thing", "")).toBe("do the thing");
    expect(applySkillPrefix("do the thing", "   \n  ")).toBe("do the thing");
  });

  it("is idempotent — re-applying the same skill body does not double-prepend", () => {
    const once = applySkillPrefix("do the thing", SKILL_BODY);
    const twice = applySkillPrefix(once, SKILL_BODY);
    expect(twice).toBe(once);
  });
});

describe("applyAllPrefixes — staged skill composition", () => {
  it("skill alone (no mode, no ultrathink): skill body + --- + user text", () => {
    expect(
      applyAllPrefixes("do the thing", "default", null, SKILL_BODY),
    ).toBe(`${SKILL_BODY}\n\n---\n\ndo the thing`);
  });

  it("skill + ask mode: ASK wrapper above skill above user text", () => {
    expect(applyAllPrefixes("do the thing", "ask", null, SKILL_BODY)).toBe(
      `${ASK_WRAPPER}\n\n${SKILL_BODY}\n\n---\n\ndo the thing`,
    );
  });

  it("skill + debug mode: DEBUG wrapper above skill above user text", () => {
    expect(applyAllPrefixes("do the thing", "debug", null, SKILL_BODY)).toBe(
      `${DEBUG_WRAPPER}\n\n${SKILL_BODY}\n\n---\n\ndo the thing`,
    );
  });

  it("skill + ultrathink + ask: full stack in locked order", () => {
    expect(
      applyAllPrefixes("do the thing", "ask", "ultrathink", SKILL_BODY),
    ).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${ASK_WRAPPER}\n\n${SKILL_BODY}\n\n---\n\ndo the thing`,
    );
  });

  it("skill + ultrathink + debug: full stack in locked order", () => {
    expect(
      applyAllPrefixes("crash on submit", "debug", "ultrathink", SKILL_BODY),
    ).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${DEBUG_WRAPPER}\n\n${SKILL_BODY}\n\n---\n\ncrash on submit`,
    );
  });

  it("skill + plan mode: skill applied (Plan has no wrapper); ultrathink lands on top", () => {
    expect(
      applyAllPrefixes("design X", "plan", "ultrathink", SKILL_BODY),
    ).toBe(`${ULTRATHINK_PROMPT_PREFIX}${SKILL_BODY}\n\n---\n\ndesign X`);
  });

  it("no skill (null) keeps existing behavior unchanged", () => {
    expect(applyAllPrefixes("hello", "default", null, null)).toBe("hello");
    expect(applyAllPrefixes("ask q", "ask", "ultrathink", null)).toBe(
      `${ULTRATHINK_PROMPT_PREFIX}${ASK_WRAPPER}\n\nask q`,
    );
  });

  it("empty skill body falls through to existing behavior", () => {
    expect(applyAllPrefixes("hello", "default", null, "")).toBe("hello");
    expect(applyAllPrefixes("hello", "ask", null, "  \n  ")).toBe(
      `${ASK_WRAPPER}\n\nhello`,
    );
  });

  it("idempotent under repeated application when only the skill is in play", () => {
    // Cross-layer idempotency (skill + mode + ultrathink stacked) isn't
    // achievable without each layer reaching into the others; the send
    // path only ever calls this fn once per turn so it isn't needed.
    // The per-layer guarantee (applySkillPrefix on its own output) is
    // covered above and is sufficient for the real call site.
    const once = applyAllPrefixes("do the thing", "default", null, SKILL_BODY);
    const twice = applyAllPrefixes(once, "default", null, SKILL_BODY);
    expect(twice).toBe(once);
  });
});
