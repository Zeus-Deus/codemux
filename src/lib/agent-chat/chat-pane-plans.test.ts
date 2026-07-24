import { describe, it, expect } from "vitest";
import type { ChatModelInfo, ProviderChatCapabilities } from "@/tauri/types";

import {
  planCapabilityCompatReset,
  planEffortChange,
  planModelChange,
  planPermissionModeChange,
  planSubmit,
} from "./chat-pane-plans";

const CLAUDE_CAPS: ProviderChatCapabilities = {
  models: [],
  effort_granularity: "per_session",
  effort_label_map: {},
  permission_modes: [
    {
      value: "default",
      label: "Supervised",
      description: "ask first",
      is_default: false,
    },
    {
      value: "acceptEdits",
      label: "Auto-accept edits",
      description: "auto edits",
      is_default: false,
    },
    {
      value: "bypassPermissions",
      label: "Full access",
      description: "no prompts",
      is_default: true,
    },
  ],
  default_permission_mode: "bypassPermissions",
  permission_granularity: "per_session",
};

const CODEX_CAPS: ProviderChatCapabilities = {
  models: [],
  effort_granularity: "per_turn",
  effort_label_map: {},
  permission_modes: [
    {
      value: "read-only",
      label: "Read only",
      description: "",
      is_default: false,
    },
    {
      value: "workspace-write",
      label: "Workspace write",
      description: "",
      is_default: false,
    },
    {
      value: "danger-full-access",
      label: "Full access",
      description: "",
      is_default: true,
    },
  ],
  default_permission_mode: "danger-full-access",
  permission_granularity: "per_session",
};

const EMPTY_PERM_CAPS: ProviderChatCapabilities = {
  models: [],
  effort_granularity: "per_turn",
  effort_label_map: {},
  permission_modes: [],
  default_permission_mode: null,
  permission_granularity: "per_turn",
};

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
  supports_images: true,
  sub_provider: null,
  is_free: false,
};

const HAIKU: ChatModelInfo = {
  ...OPUS_4_7,
  id: "claude-haiku-4-5",
  effort_levels: [],
  default_effort: null,
  prompt_injected_effort_levels: [],
  context_window_options: [],
};

const GPT_54: ChatModelInfo = {
  id: "gpt-5.4",
  label: "GPT-5.4",
  description: null,
  effort_levels: ["low", "medium", "high", "xhigh"],
  default_effort: "medium",
  prompt_injected_effort_levels: [],
  context_window_options: [],
  supports_adaptive_thinking: false,
  supports_thinking_toggle: false,
  supports_fast_mode: false,
  supports_images: true,
  sub_provider: null,
  is_free: false,
};

// ─── planEffortChange ───

describe("planEffortChange", () => {
  it("returns null when model is null (capabilities unavailable)", () => {
    const plan = planEffortChange({
      nextEffort: "high",
      model: null,
      currentDraft: "hi",
      provider: "claude",
    });
    expect(plan).toBeNull();
  });

  it("ultrathink into an empty draft seeds the prefix", () => {
    const plan = planEffortChange({
      nextEffort: "ultrathink",
      model: OPUS_4_7,
      currentDraft: "",
      provider: "claude",
    });
    expect(plan).not.toBeNull();
    expect(plan!.updateDraft).toEqual({
      kind: "prepend",
      nextDraft: "Ultrathink:\n",
    });
    expect(plan!.setEffort).toBeNull();
    expect(plan!.restart).toBe(false);
  });

  it("ultrathink into a non-empty draft prepends the prefix", () => {
    const plan = planEffortChange({
      nextEffort: "ultrathink",
      model: OPUS_4_7,
      currentDraft: "Find the bug",
      provider: "claude",
    });
    expect(plan!.updateDraft).toEqual({
      kind: "prepend",
      nextDraft: "Ultrathink:\nFind the bug",
    });
    expect(plan!.setEffort).toBeNull();
    expect(plan!.restart).toBe(false);
  });

  it("ultrathink is idempotent when the prefix is already present", () => {
    const plan = planEffortChange({
      nextEffort: "ultrathink",
      model: OPUS_4_7,
      currentDraft: "Ultrathink:\nAlready set",
      provider: "claude",
    });
    expect(plan!.updateDraft).toEqual({
      kind: "prepend",
      nextDraft: "Ultrathink:\nAlready set",
    });
  });

  it("normal effort, draft has prefix → strip + set effort + restart (Claude)", () => {
    const plan = planEffortChange({
      nextEffort: "high",
      model: OPUS_4_7,
      currentDraft: "Ultrathink:\nWork harder",
      provider: "claude",
    });
    expect(plan!.updateDraft).toEqual({
      kind: "strip",
      nextDraft: "Work harder",
    });
    expect(plan!.setEffort).toBe("high");
    expect(plan!.restart).toBe(true);
  });

  it("normal effort, draft has no prefix → set effort + restart (Claude)", () => {
    const plan = planEffortChange({
      nextEffort: "xhigh",
      model: OPUS_4_7,
      currentDraft: "Regular prompt",
      provider: "claude",
    });
    expect(plan!.updateDraft).toBeNull();
    expect(plan!.setEffort).toBe("xhigh");
    expect(plan!.restart).toBe(true);
  });

  it("normal effort on Codex → set effort, NO restart (per-turn)", () => {
    const plan = planEffortChange({
      nextEffort: "high",
      model: GPT_54,
      currentDraft: "Do it",
      provider: "codex",
    });
    expect(plan!.setEffort).toBe("high");
    expect(plan!.restart).toBe(false);
  });

  it("strip-and-set on Codex skips the restart", () => {
    const plan = planEffortChange({
      nextEffort: "medium",
      model: GPT_54,
      currentDraft: "Ultrathink:\nCodex ignores this",
      provider: "codex",
    });
    expect(plan!.updateDraft).toEqual({
      kind: "strip",
      nextDraft: "Codex ignores this",
    });
    expect(plan!.setEffort).toBe("medium");
    expect(plan!.restart).toBe(false);
  });

  it("case-insensitive prefix strip", () => {
    const plan = planEffortChange({
      nextEffort: "low",
      model: OPUS_4_7,
      currentDraft: "ultrathink:   do stuff",
      provider: "claude",
    });
    expect(plan!.updateDraft).toEqual({
      kind: "strip",
      nextDraft: "do stuff",
    });
  });
});

// ─── planModelChange ───

describe("planModelChange", () => {
  it("returns undefined fields when new model is null", () => {
    const plan = planModelChange({
      newModel: null,
      currentEffort: "xhigh",
      currentContextWindow: "1m",
    });
    expect(plan).toEqual({
      resetEffort: undefined,
      resetContextWindow: undefined,
      resetFastMode: undefined,
    });
  });

  it("leaves effort + contextWindow alone when both are compatible", () => {
    const plan = planModelChange({
      newModel: OPUS_4_7,
      currentEffort: "xhigh",
      currentContextWindow: "200k",
    });
    expect(plan.resetEffort).toBeUndefined();
    expect(plan.resetContextWindow).toBeUndefined();
  });

  it("resets effort to the new model's default when orphaned", () => {
    const plan = planModelChange({
      newModel: GPT_54,
      currentEffort: "ultrathink", // not in GPT-5.4's levels
      currentContextWindow: null,
    });
    expect(plan.resetEffort).toBe("medium"); // GPT-5.4 default
  });

  it("Claude → Haiku clears effort (Haiku has no effort levels)", () => {
    const plan = planModelChange({
      newModel: HAIKU,
      currentEffort: "xhigh",
      currentContextWindow: "1m",
    });
    expect(plan.resetEffort).toBeNull();
    expect(plan.resetContextWindow).toBeNull();
  });

  it("contextWindow falls back to the new model's default when orphaned", () => {
    const noCtxModel = { ...OPUS_4_7, context_window_options: [] };
    const plan = planModelChange({
      newModel: noCtxModel,
      currentEffort: "xhigh",
      currentContextWindow: "1m",
    });
    expect(plan.resetContextWindow).toBeNull();
  });

  it("keeps ultrathink effort through model switch when new model supports it", () => {
    const plan = planModelChange({
      newModel: OPUS_4_7, // supports ultrathink (prompt_injected)
      currentEffort: "ultrathink",
      currentContextWindow: null,
    });
    expect(plan.resetEffort).toBeUndefined();
  });

  it("turns fast mode off when the new model does not support it", () => {
    const plan = planModelChange({
      newModel: HAIKU,
      currentEffort: null,
      currentContextWindow: null,
      currentFastMode: true,
    });
    expect(plan.resetFastMode).toBe(false);
  });
});

// ─── planSubmit ───

describe("planSubmit", () => {
  it("Claude non-ultrathink passes text through unchanged", () => {
    const plan = planSubmit({
      rawText: "hello",
      provider: "claude",
      effort: "high",
    });
    expect(plan.text).toBe("hello");
    expect(plan.effortOverride).toBeNull();
  });

  it("Claude + ultrathink prepends the prefix (belt-and-braces)", () => {
    const plan = planSubmit({
      rawText: "investigate",
      provider: "claude",
      effort: "ultrathink",
    });
    expect(plan.text).toBe("Ultrathink:\ninvestigate");
    expect(plan.effortOverride).toBeNull();
  });

  it("Claude + ultrathink is idempotent when text already has the prefix", () => {
    const plan = planSubmit({
      rawText: "Ultrathink:\ninvestigate",
      provider: "claude",
      effort: "ultrathink",
    });
    expect(plan.text).toBe("Ultrathink:\ninvestigate");
  });

  it("Codex passes the effort as effort_override (per-turn)", () => {
    const plan = planSubmit({
      rawText: "build it",
      provider: "codex",
      effort: "high",
    });
    expect(plan.text).toBe("build it");
    expect(plan.effortOverride).toBe("high");
  });

  it("Codex with null effort passes null effort_override", () => {
    const plan = planSubmit({
      rawText: "ship",
      provider: "codex",
      effort: null,
    });
    expect(plan.effortOverride).toBeNull();
  });

  it("Codex never prepends ultrathink even when effort is 'ultrathink'", () => {
    // Shouldn't happen in practice (Codex has no ultrathink), but the
    // submit plan should NOT mutate text for the Codex branch.
    const plan = planSubmit({
      rawText: "hi",
      provider: "codex",
      effort: "ultrathink",
    });
    expect(plan.text).toBe("hi");
    expect(plan.effortOverride).toBe("ultrathink");
  });
});

// ─── planPermissionModeChange ───

describe("planPermissionModeChange", () => {
  it("returns null when capabilities are unavailable", () => {
    const plan = planPermissionModeChange({
      nextMode: "default",
      capabilities: null,
    });
    expect(plan).toBeNull();
  });

  it("returns null when the mode isn't in the provider's table", () => {
    const plan = planPermissionModeChange({
      nextMode: "not-a-mode",
      capabilities: CLAUDE_CAPS,
    });
    expect(plan).toBeNull();
  });

  it("Claude PerSession → restart=true", () => {
    const plan = planPermissionModeChange({
      nextMode: "default",
      capabilities: CLAUDE_CAPS,
    });
    expect(plan).toEqual({ setPermissionMode: "default", restart: true });
  });

  it("Codex PerSession (MVP wiring) → restart=true", () => {
    const plan = planPermissionModeChange({
      nextMode: "read-only",
      capabilities: CODEX_CAPS,
    });
    expect(plan).toEqual({ setPermissionMode: "read-only", restart: true });
  });

  it("hypothetical PerTurn provider → restart=false", () => {
    const perTurnCaps: ProviderChatCapabilities = {
      ...CODEX_CAPS,
      permission_granularity: "per_turn",
    };
    const plan = planPermissionModeChange({
      nextMode: "read-only",
      capabilities: perTurnCaps,
    });
    expect(plan).toEqual({ setPermissionMode: "read-only", restart: false });
  });
});

// ─── planCapabilityCompatReset ───

describe("planCapabilityCompatReset", () => {
  it("returns undefined when capabilities are null (nothing to reset against)", () => {
    const plan = planCapabilityCompatReset({
      capabilities: null,
      currentPermissionMode: "default",
    });
    expect(plan.resetPermissionMode).toBeUndefined();
  });

  it("seeds the default when the slice has no permission mode yet", () => {
    const plan = planCapabilityCompatReset({
      capabilities: CLAUDE_CAPS,
      currentPermissionMode: null,
    });
    expect(plan.resetPermissionMode).toBe("bypassPermissions");
  });

  it("leaves a compatible value alone", () => {
    const plan = planCapabilityCompatReset({
      capabilities: CLAUDE_CAPS,
      currentPermissionMode: "acceptEdits",
    });
    expect(plan.resetPermissionMode).toBeUndefined();
  });

  it("resets an orphaned value to the new provider's default", () => {
    // Thread was on Claude's "acceptEdits", now switching to Codex —
    // that value isn't valid for Codex.
    const plan = planCapabilityCompatReset({
      capabilities: CODEX_CAPS,
      currentPermissionMode: "acceptEdits",
    });
    expect(plan.resetPermissionMode).toBe("danger-full-access");
  });

  it("resets to null when the new provider has no modes at all", () => {
    const plan = planCapabilityCompatReset({
      capabilities: EMPTY_PERM_CAPS,
      currentPermissionMode: "default",
    });
    expect(plan.resetPermissionMode).toBeNull();
  });
});
