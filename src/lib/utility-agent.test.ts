import { afterEach, describe, expect, it } from "vitest";

import {
  resolveAutoUtilitySelection,
  utilityEffortFor,
  utilitySelectionFromStores,
} from "./utility-agent";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  ProviderChatCapabilities,
} from "@/tauri/types";

function model(
  id: string,
  overrides: Partial<ChatModelInfo> = {},
): ChatModelInfo {
  return {
    id,
    label: id,
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
    is_free: false,
    ...overrides,
  };
}

function capabilities(models: ChatModelInfo[]): ProviderChatCapabilities {
  return {
    models,
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

function setProvider(
  provider: AgentChatProviderKind,
  caps: ProviderChatCapabilities | null,
  error: string | null = null,
) {
  useProviderCapabilities.setState({
    [provider]: caps,
    [`${provider}Error`]: error,
  });
}

afterEach(() => {
  useProviderCapabilities.setState({
    claude: null,
    codex: null,
    cursor: null,
    grok: null,
    opencode: null,
    claudeError: null,
    codexError: null,
    cursorError: null,
    grokError: null,
    opencodeError: null,
  });
  useSettingsStore.setState({ settings: {} });
});

describe("Utility agent selection", () => {
  it("prefers Codex Luna at low effort when it is available", () => {
    setProvider(
      "codex",
      capabilities([
        model("gpt-5.6-sol", { effort_levels: ["medium"] }),
        model("gpt-5.6-luna", {
          effort_levels: ["low", "medium"],
          default_effort: "medium",
        }),
      ]),
    );
    setProvider("claude", capabilities([model("claude-haiku-4-5")]));

    expect(resolveAutoUtilitySelection()).toEqual({
      provider: "codex",
      model: "gpt-5.6-luna",
      effort: "low",
    });
  });

  it("falls back to Claude Haiku when Codex is unavailable", () => {
    setProvider("codex", null, "codex_not_installed");
    setProvider(
      "claude",
      capabilities([model("claude-sonnet-5"), model("claude-haiku-4-5")]),
    );

    expect(resolveAutoUtilitySelection()).toEqual({
      provider: "claude",
      model: "claude-haiku-4-5",
      effort: null,
    });
  });

  it("only auto-selects OpenCode models explicitly marked free", () => {
    setProvider(
      "opencode",
      capabilities([model("paid"), model("free", { is_free: true })]),
    );
    expect(resolveAutoUtilitySelection()).toEqual({
      provider: "opencode",
      model: "free",
      effort: null,
    });

    setProvider("opencode", capabilities([model("paid")]));
    expect(resolveAutoUtilitySelection()).toBeNull();
  });

  it("uses a valid custom selection and rejects an unset model", () => {
    useSettingsStore.setState({
      settings: {
        "ai.utility.mode": "custom",
        "ai.utility.provider": "claude",
        "ai.utility.model": "claude-haiku-4-5",
        "ai.utility.effort": "low",
      },
    });
    expect(utilitySelectionFromStores()).toEqual({
      provider: "claude",
      model: "claude-haiku-4-5",
      effort: "low",
    });

    useSettingsStore.setState({
      settings: {
        "ai.utility.mode": "custom",
        "ai.utility.provider": "claude",
        "ai.utility.model": "",
      },
    });
    expect(utilitySelectionFromStores()).toBeNull();
  });

  it("rejects a Cursor custom selection the utility backend cannot run", () => {
    // `utility_ai.rs::build_invocation` only knows codex/claude/opencode;
    // a stored Cursor pick would fail every commit message, handoff
    // summary, and session-context pass with
    // `utility_provider_unsupported`. Reporting "no utility model" is the
    // same answer any other unrunnable custom pick gets.
    setProvider("claude", capabilities([model("claude-haiku-4-5")]));
    useSettingsStore.setState({
      settings: {
        "ai.utility.mode": "custom",
        "ai.utility.provider": "cursor",
        "ai.utility.model": "auto",
      },
    });
    expect(utilitySelectionFromStores()).toBeNull();
  });

  it("pins Codex Luna to low effort and leaves other models on their default", () => {
    const luna = model("gpt-5.6-luna", {
      effort_levels: ["low", "medium"],
      default_effort: "medium",
    });
    expect(utilityEffortFor("codex", luna.id, luna)).toBe("low");
    const sol = model("gpt-5.6-sol", {
      effort_levels: ["medium", "high"],
      default_effort: "medium",
    });
    expect(utilityEffortFor("codex", sol.id, sol)).toBe("medium");
    // A Luna variant without a low tier keeps whatever the provider defaults
    // to rather than sending an effort the CLI would reject.
    const lunaNoLow = model("gpt-5.6-luna-x", {
      effort_levels: ["medium"],
      default_effort: "medium",
    });
    expect(utilityEffortFor("codex", lunaNoLow.id, lunaNoLow)).toBe("medium");
    expect(utilityEffortFor("claude", "claude-haiku-4-5", null)).toBeNull();
  });
});
