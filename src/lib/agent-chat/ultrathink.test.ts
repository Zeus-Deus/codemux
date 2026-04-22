import { describe, it, expect } from "vitest";
import {
  applyClaudePromptEffortPrefix,
  isClaudeUltrathinkPrompt,
  hasUltrathinkInBodyText,
  stripClaudeUltrathinkPrefix,
  ULTRATHINK_PROMPT_PREFIX,
} from "./ultrathink";

describe("applyClaudePromptEffortPrefix", () => {
  it("prepends on ultrathink effort", () => {
    expect(applyClaudePromptEffortPrefix("Investigate", "ultrathink")).toBe(
      "Ultrathink:\nInvestigate",
    );
  });

  it("is idempotent — does not double-prepend", () => {
    const once = applyClaudePromptEffortPrefix("Investigate", "ultrathink");
    const twice = applyClaudePromptEffortPrefix(once, "ultrathink");
    expect(twice).toBe("Ultrathink:\nInvestigate");
  });

  it("no-op for every non-ultrathink effort", () => {
    for (const effort of ["low", "medium", "high", "xhigh", "max", null, undefined, ""]) {
      expect(applyClaudePromptEffortPrefix("hello", effort)).toBe("hello");
    }
  });

  it("trims whitespace around the body", () => {
    expect(applyClaudePromptEffortPrefix("   Investigate  ", "ultrathink")).toBe(
      "Ultrathink:\nInvestigate",
    );
  });

  it("returns empty for empty / whitespace-only inputs", () => {
    expect(applyClaudePromptEffortPrefix("", "ultrathink")).toBe("");
    expect(applyClaudePromptEffortPrefix("   ", "ultrathink")).toBe("");
  });

  it("uses the exported prefix constant verbatim", () => {
    expect(ULTRATHINK_PROMPT_PREFIX).toBe("Ultrathink:\n");
  });
});

describe("isClaudeUltrathinkPrompt", () => {
  it("detects the literal prefix", () => {
    expect(isClaudeUltrathinkPrompt("Ultrathink:\nInvestigate")).toBe(true);
  });

  it("detects ultrathink anywhere as a whole word", () => {
    expect(isClaudeUltrathinkPrompt("Please ultrathink this")).toBe(true);
    expect(isClaudeUltrathinkPrompt("ULTRATHINK on this")).toBe(true);
  });

  it("does not match substrings", () => {
    expect(isClaudeUltrathinkPrompt("preultrathinker tips")).toBe(false);
    expect(isClaudeUltrathinkPrompt("ultrathinkalike")).toBe(false);
  });

  it("returns false for non-string inputs", () => {
    expect(isClaudeUltrathinkPrompt(null)).toBe(false);
    expect(isClaudeUltrathinkPrompt(undefined)).toBe(false);
  });
});

describe("stripClaudeUltrathinkPrefix", () => {
  it("strips the leading prefix", () => {
    expect(stripClaudeUltrathinkPrefix("Ultrathink:\nHello")).toBe("Hello");
  });

  it("is case-insensitive on the prefix", () => {
    expect(stripClaudeUltrathinkPrefix("ultrathink:   Hello")).toBe("Hello");
  });

  it("leaves non-prefixed prompts alone", () => {
    expect(stripClaudeUltrathinkPrefix("Hello world")).toBe("Hello world");
  });

  it("leaves mid-body ultrathink mentions alone", () => {
    expect(stripClaudeUltrathinkPrefix("Please ultrathink this")).toBe(
      "Please ultrathink this",
    );
  });
});

describe("hasUltrathinkInBodyText", () => {
  it("false when prompt only has the canonical prefix", () => {
    expect(hasUltrathinkInBodyText("Ultrathink:\nHello")).toBe(false);
  });

  it("true when ultrathink appears in the body after stripping the prefix", () => {
    expect(hasUltrathinkInBodyText("Ultrathink:\nPlease ultrathink this")).toBe(
      true,
    );
  });

  it("true when ultrathink is mid-body with no prefix", () => {
    expect(hasUltrathinkInBodyText("can you ultrathink it")).toBe(true);
  });

  it("false for null / undefined / empty", () => {
    expect(hasUltrathinkInBodyText(null)).toBe(false);
    expect(hasUltrathinkInBodyText(undefined)).toBe(false);
    expect(hasUltrathinkInBodyText("")).toBe(false);
  });
});
