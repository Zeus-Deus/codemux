import { beforeEach, describe, expect, it } from "vitest";

import {
  capabilityDefaults,
  defaultModelId,
  modelLabel,
  modelsForProvider,
} from "./capability-defaults";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import type {
  ChatModelInfo,
  ProviderChatCapabilities,
} from "@/tauri/types";

function makeModel(overrides: Partial<ChatModelInfo>): ChatModelInfo {
  return {
    id: "model-x",
    label: "Model X",
    description: null,
    effort_levels: [],
    default_effort: null,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: false,
    supports_fast_mode: false,
    supports_images: false,
    sub_provider: null,
    ...overrides,
  };
}

function makeClaudeCaps(
  models: ChatModelInfo[],
): ProviderChatCapabilities {
  return {
    models,
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

function resetCaps() {
  useProviderCapabilities.setState({
    claude: null,
    codex: null,
    claudeError: null,
    codexError: null,
    loaded: false,
  });
}

describe("capability-defaults", () => {
  beforeEach(() => {
    resetCaps();
  });

  describe("defaultModelId", () => {
    it("returns the capabilities store's first model when hydrated", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({ id: "custom-model-abc" }),
          makeModel({ id: "claude-opus-4-7" }),
        ]),
      });
      expect(defaultModelId("claude")).toBe("custom-model-abc");
    });

    it("falls back to the hardcoded Claude default when caps are unhydrated", () => {
      expect(defaultModelId("claude")).toBe("claude-opus-4-7");
    });

    it("falls back to the hardcoded Codex default when caps are unhydrated", () => {
      expect(defaultModelId("codex")).toBe("gpt-5.4");
    });
  });

  describe("modelsForProvider", () => {
    it("returns a [{id, label}] projection of the capabilities list", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({ id: "a", label: "Alpha" }),
          makeModel({ id: "b", label: "Beta" }),
        ]),
      });
      expect(modelsForProvider("claude")).toEqual([
        { id: "a", label: "Alpha" },
        { id: "b", label: "Beta" },
      ]);
    });

    it("returns an empty list when caps are unhydrated", () => {
      expect(modelsForProvider("claude")).toEqual([]);
    });
  });

  describe("modelLabel", () => {
    it("returns the label for a known model id", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([makeModel({ id: "a", label: "Alpha" })]),
      });
      expect(modelLabel("claude", "a")).toBe("Alpha");
    });

    it("falls back to the raw id when the model is unknown", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([makeModel({ id: "a", label: "Alpha" })]),
      });
      expect(modelLabel("claude", "mystery")).toBe("mystery");
    });

    it("falls back to the raw id when caps are unhydrated", () => {
      expect(modelLabel("claude", "anything")).toBe("anything");
    });
  });

  describe("capabilityDefaults", () => {
    it("pulls default_effort and the is_default context window from the model", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({
            id: "claude-opus-4-7",
            default_effort: "xhigh",
            context_window_options: [
              { value: "200k", label: "200k", is_default: true },
              { value: "1m", label: "1M", is_default: false },
            ],
          }),
        ]),
      });

      const defaults = capabilityDefaults("claude", "claude-opus-4-7");
      expect(defaults.model).toBe("claude-opus-4-7");
      expect(defaults.effort).toBe("xhigh");
      expect(defaults.contextWindow).toBe("200k");
      expect(defaults.permissionMode).toBe("bypassPermissions");
    });

    it("returns the highest-is-default context window (production rule)", () => {
      // Mirrors `capabilities.rs`'s post-Stage-C flip: on multi-
      // option models, 1m is `is_default: true`. The helper should
      // propagate that without any extra logic.
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({
            id: "claude-opus-4-7",
            default_effort: "xhigh",
            context_window_options: [
              { value: "200k", label: "200k", is_default: false },
              { value: "1m", label: "1M", is_default: true },
            ],
          }),
        ]),
      });
      const defaults = capabilityDefaults("claude", "claude-opus-4-7");
      expect(defaults.contextWindow).toBe("1m");
    });

    it("falls back to the first context window option when none flagged as default", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({
            id: "claude-opus-4-7",
            default_effort: "high",
            context_window_options: [
              { value: "200k", label: "200k", is_default: false },
              { value: "1m", label: "1M", is_default: false },
            ],
          }),
        ]),
      });
      const defaults = capabilityDefaults("claude", "claude-opus-4-7");
      expect(defaults.contextWindow).toBe("200k");
    });

    it("returns null effort / contextWindow when the model has none of those axes", () => {
      useProviderCapabilities.setState({
        claude: makeClaudeCaps([
          makeModel({
            id: "claude-haiku-4-5",
            default_effort: null,
            context_window_options: [],
          }),
        ]),
      });
      const defaults = capabilityDefaults("claude", "claude-haiku-4-5");
      expect(defaults.effort).toBeNull();
      expect(defaults.contextWindow).toBeNull();
    });

    it("returns sensible null-ish defaults when caps are unhydrated", () => {
      const defaults = capabilityDefaults("claude", "claude-opus-4-7");
      expect(defaults.model).toBe("claude-opus-4-7");
      expect(defaults.effort).toBeNull();
      expect(defaults.contextWindow).toBeNull();
      expect(defaults.permissionMode).toBe("bypassPermissions");
    });
  });
});
