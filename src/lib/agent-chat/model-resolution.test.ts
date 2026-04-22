import { describe, it, expect } from "vitest";
import type { ChatModelInfo } from "@/tauri/types";
import {
  resolveEffort,
  effortForApi,
  resolveContextWindow,
  resolveClaudeApiModelId,
  getDefaultContextWindow,
  hasContextWindowOption,
  hasEffortLevel,
  isPromptInjectedEffort,
} from "./model-resolution";

const OPUS_4_7: ChatModelInfo = {
  id: "claude-opus-4-7",
  label: "Claude Opus 4.7",
  description: null,
  effort_levels: ["low", "medium", "high", "xhigh", "max"],
  default_effort: "xhigh",
  prompt_injected_effort_levels: ["ultrathink"],
  context_window_options: [
    { value: "200k", label: "200k", is_default: true },
    { value: "1m", label: "1M", is_default: false },
  ],
  supports_adaptive_thinking: true,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
};

const HAIKU: ChatModelInfo = {
  id: "claude-haiku-4-5",
  label: "Claude Haiku 4.5",
  description: null,
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: true,
  supports_fast_mode: false,
};

describe("resolveEffort", () => {
  it("returns null when model is null", () => {
    expect(resolveEffort(null, "high")).toBeNull();
  });

  it("returns null when model has no effort levels (Haiku)", () => {
    expect(resolveEffort(HAIKU, "high")).toBeNull();
  });

  it("keeps a natively-supported value", () => {
    expect(resolveEffort(OPUS_4_7, "xhigh")).toBe("xhigh");
  });

  it("keeps a prompt-injected value (ultrathink)", () => {
    expect(resolveEffort(OPUS_4_7, "ultrathink")).toBe("ultrathink");
  });

  it("falls back to default when raw is unsupported", () => {
    expect(resolveEffort(OPUS_4_7, "invalid-level")).toBe("xhigh");
  });

  it("falls back to default when raw is empty / null / whitespace", () => {
    expect(resolveEffort(OPUS_4_7, null)).toBe("xhigh");
    expect(resolveEffort(OPUS_4_7, "")).toBe("xhigh");
    expect(resolveEffort(OPUS_4_7, "   ")).toBe("xhigh");
  });
});

describe("effortForApi", () => {
  it("returns null for null model or null effort", () => {
    expect(effortForApi(null, "high")).toBeNull();
    expect(effortForApi(OPUS_4_7, null)).toBeNull();
  });

  it("passes through natively-supported values", () => {
    expect(effortForApi(OPUS_4_7, "xhigh")).toBe("xhigh");
  });

  it("substitutes default when effort is ultrathink — ultrathink never reaches the SDK", () => {
    expect(effortForApi(OPUS_4_7, "ultrathink")).toBe("xhigh");
  });
});

describe("resolveContextWindow", () => {
  it("returns null when model has no options (Haiku)", () => {
    expect(resolveContextWindow(HAIKU, "1m")).toBeNull();
  });

  it("keeps a supported value", () => {
    expect(resolveContextWindow(OPUS_4_7, "1m")).toBe("1m");
  });

  it("falls back to default when raw is unsupported", () => {
    expect(resolveContextWindow(OPUS_4_7, "42m")).toBe("200k");
  });

  it("falls back to default when raw is null", () => {
    expect(resolveContextWindow(OPUS_4_7, null)).toBe("200k");
  });
});

describe("resolveClaudeApiModelId", () => {
  it("appends [1m] when contextWindow is '1m'", () => {
    expect(resolveClaudeApiModelId("claude-opus-4-7", "1m")).toBe(
      "claude-opus-4-7[1m]",
    );
  });

  it("passes through for '200k' / null / undefined / empty", () => {
    expect(resolveClaudeApiModelId("claude-opus-4-7", "200k")).toBe(
      "claude-opus-4-7",
    );
    expect(resolveClaudeApiModelId("claude-opus-4-7", null)).toBe(
      "claude-opus-4-7",
    );
    expect(resolveClaudeApiModelId("claude-opus-4-7", undefined)).toBe(
      "claude-opus-4-7",
    );
    expect(resolveClaudeApiModelId("claude-opus-4-7", "")).toBe(
      "claude-opus-4-7",
    );
  });
});

describe("small predicates", () => {
  it("getDefaultContextWindow", () => {
    expect(getDefaultContextWindow(OPUS_4_7)).toBe("200k");
    expect(getDefaultContextWindow(HAIKU)).toBeNull();
  });

  it("hasContextWindowOption", () => {
    expect(hasContextWindowOption(OPUS_4_7, "1m")).toBe(true);
    expect(hasContextWindowOption(OPUS_4_7, "42m")).toBe(false);
  });

  it("hasEffortLevel", () => {
    expect(hasEffortLevel(OPUS_4_7, "xhigh")).toBe(true);
    expect(hasEffortLevel(OPUS_4_7, "ultrathink")).toBe(false);
  });

  it("isPromptInjectedEffort", () => {
    expect(isPromptInjectedEffort(OPUS_4_7, "ultrathink")).toBe(true);
    expect(isPromptInjectedEffort(OPUS_4_7, "xhigh")).toBe(false);
    expect(isPromptInjectedEffort(HAIKU, "ultrathink")).toBe(false);
  });
});
