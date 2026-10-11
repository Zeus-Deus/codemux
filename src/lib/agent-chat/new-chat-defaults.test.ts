import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(),
  dbSetSetting: vi.fn(() => Promise.resolve()),
}));

import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import type { ChatModelInfo, ProviderChatCapabilities } from "@/tauri/types";
import { resolveNewChatModelDefaults } from "./new-chat-defaults";

function model(id: string, effort_levels: string[] = []): ChatModelInfo {
  return {
    id,
    label: id,
    description: null,
    effort_levels,
    default_effort: effort_levels[0] ?? null,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: false,
    supports_fast_mode: false,
    supports_images: false,
    sub_provider: null,
    is_free: false,
  };
}

function caps(models: ChatModelInfo[]): ProviderChatCapabilities {
  return {
    models,
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

function choose(settings: Record<string, string>) {
  useSettingsStore.setState({ settings });
}

describe("resolveNewChatModelDefaults", () => {
  beforeEach(() => {
    useProviderCapabilities.setState({
      claude: caps([model("claude-opus", ["low", "high"])]),
      codex: caps([model("gpt-a", ["low", "high"]), model("gpt-b", ["medium"])]),
    });
  });

  afterEach(() => {
    useSettingsStore.setState({ settings: {} });
    useProviderCapabilities.setState({ claude: null, codex: null });
  });

  it("starts on Claude's default model when nothing is chosen", () => {
    choose({});
    expect(resolveNewChatModelDefaults()).toEqual({
      provider: "claude",
      model: "claude-opus",
      effort: null,
    });
  });

  it("uses the chosen provider, model and effort", () => {
    choose({
      "chat.default_provider": "codex",
      "chat.default_model": "gpt-a",
      "chat.default_effort": "high",
    });
    expect(resolveNewChatModelDefaults()).toEqual({
      provider: "codex",
      model: "gpt-a",
      effort: "high",
    });
  });

  it("drops an effort the chosen model does not offer", () => {
    choose({
      "chat.default_provider": "codex",
      "chat.default_model": "gpt-b",
      "chat.default_effort": "high",
    });
    expect(resolveNewChatModelDefaults().effort).toBeNull();
  });

  it("falls back to the provider's default model when the saved one is gone", () => {
    choose({
      "chat.default_provider": "codex",
      "chat.default_model": "gpt-retired",
      "chat.default_effort": "high",
    });
    expect(resolveNewChatModelDefaults()).toEqual({
      provider: "codex",
      model: "gpt-a",
      effort: null,
    });
  });

  it("ignores providers that cannot be a new-chat default", () => {
    choose({
      "chat.default_provider": "hermes",
      "chat.default_model": "profile_default",
    });
    expect(resolveNewChatModelDefaults().provider).toBe("claude");
  });
});
