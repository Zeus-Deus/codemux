import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: vi.fn(),
  dbSetSetting: vi.fn(() => Promise.resolve()),
}));

import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { emptyHealthSlot, useProviderHealth } from "@/stores/provider-health-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  ProviderChatCapabilities,
  ProviderHealthReport,
} from "@/tauri/types";
import { defaultModelId } from "./capability-defaults";
import {
  firstUsableProvider,
  isAutomaticNewChatProvider,
  resolveNewChatModelDefaults,
} from "./new-chat-defaults";

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

function resetHealth() {
  useProviderHealth.setState({
    slots: {
      claude: emptyHealthSlot(), codex: emptyHealthSlot(), cursor: emptyHealthSlot(),
      grok: emptyHealthSlot(), hermes: emptyHealthSlot(), acp: emptyHealthSlot(),
      opencode: emptyHealthSlot(),
    },
  });
}

/** Cache a probe answer for each provider: "ready", "signed-out",
 *  "unverified" (installed, sign-in can't be checked), "missing", or
 *  "failed" (the probe itself failed at the IPC layer). */
function probed(
  answers: Partial<
    Record<AgentChatProviderKind, "ready" | "signed-out" | "unverified" | "missing" | "failed">
  >,
) {
  useProviderHealth.setState((state) => {
    const slots = { ...state.slots };
    for (const [provider, answer] of Object.entries(answers) as [AgentChatProviderKind, string][]) {
      const report: ProviderHealthReport = {
        provider,
        status: answer === "ready" ? "ready" : answer === "unverified" ? "warning" : "error",
        installed: answer !== "missing",
        message: null,
        version: null,
        login_command: answer === "signed-out" ? `${provider} login` : null,
      };
      slots[provider] = {
        ...emptyHealthSlot(),
        report,
        fetchedAt: 1,
        lastAttemptAt: 1,
        lastProbeFailed: answer === "failed",
      };
    }
    return { slots };
  });
}

describe("resolveNewChatModelDefaults", () => {
  beforeEach(() => {
    useChatGptStore.setState({ status: null });
    useProviderCapabilities.setState({
      claude: caps([model("claude-opus", ["low", "high"])]),
      codex: caps([model("gpt-a", ["low", "high"]), model("gpt-b", ["medium"])]),
    });
  });

  afterEach(() => {
    useChatGptStore.setState({ status: null });
    useSettingsStore.setState({ settings: {} });
    useProviderCapabilities.setState({ claude: null, codex: null });
    resetHealth();
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

  it("uses the connected plan's live catalog for Automatic and preserves custom defaults", () => {
    useChatGptStore.setState({ status: {
      phase: "connected", attemptId: null, email: null, error: null,
      profiles: [], activeProfileId: "plan", welcomePending: false, installed: true,
    } });
    choose({});
    expect(resolveNewChatModelDefaults()).toEqual({ provider: "codex", model: "gpt-a", effort: null });
    useProviderCapabilities.setState({ codex: null });
    expect(resolveNewChatModelDefaults()).toEqual({ provider: "codex", model: null, effort: null });
    choose({ "chat.default_provider": "codex" });
    expect(resolveNewChatModelDefaults()).toEqual({ provider: "codex", model: null, effort: null });
    useProviderCapabilities.setState({ codex: caps([]) });
    expect(resolveNewChatModelDefaults()).toEqual({ provider: "codex", model: null, effort: null });
    choose({ "chat.default_provider": "claude", "chat.default_model": "claude-opus" });
    expect(resolveNewChatModelDefaults()).toEqual({ provider: "claude", model: "claude-opus", effort: null });
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

  it("never starts a new chat on a provider turned off in Settings → Providers", () => {
    choose({ "chat.default_provider": "codex", "chat.default_model": "gpt-a", "providers.disabled": "codex" });
    expect(resolveNewChatModelDefaults().provider).toBe("claude");
    // Automatic skips a connected ChatGPT plan when Codex is off.
    useChatGptStore.setState({ status: {
      phase: "connected", attemptId: null, email: null, error: null,
      profiles: [], activeProfileId: "plan", welcomePending: false, installed: true,
    } });
    choose({ "providers.disabled": "codex" });
    expect(resolveNewChatModelDefaults().provider).toBe("claude");
    // With Claude off too, the next enabled provider takes over.
    choose({ "providers.disabled": "claude,codex" });
    expect(resolveNewChatModelDefaults().provider).toBe("cursor");
  });

  it("ignores providers that cannot be a new-chat default", () => {
    choose({
      "chat.default_provider": "hermes",
      "chat.default_model": "profile_default",
    });
    expect(resolveNewChatModelDefaults().provider).toBe("claude");
  });

  describe("Automatic, from the health probes", () => {
    beforeEach(() => {
      choose({});
      resetHealth();
    });

    it("starts on the first ready agent in order", () => {
      probed({ claude: "missing", codex: "signed-out", opencode: "ready", grok: "ready" });
      expect(firstUsableProvider()).toBe("opencode");
      expect(resolveNewChatModelDefaults()).toEqual({
        provider: "opencode",
        model: defaultModelId("opencode"),
        effort: null,
      });
      probed({ codex: "ready" });
      expect(resolveNewChatModelDefaults().provider).toBe("codex");
      probed({ claude: "ready" });
      expect(resolveNewChatModelDefaults().provider).toBe("claude");
    });

    it("keeps Claude when nothing is known or ready", () => {
      expect(firstUsableProvider()).toBeNull();
      expect(resolveNewChatModelDefaults().provider).toBe("claude");
      probed({ claude: "signed-out", codex: "missing", opencode: "missing" });
      expect(resolveNewChatModelDefaults().provider).toBe("claude");
    });

    it("prefers a ready agent to one it can't verify, but uses that over nothing", () => {
      probed({ claude: "unverified", codex: "signed-out", opencode: "ready" });
      expect(firstUsableProvider()).toBe("opencode");
      probed({ opencode: "missing" });
      expect(firstUsableProvider()).toBe("claude");
      probed({ claude: "missing", cursor: "unverified" });
      expect(resolveNewChatModelDefaults().provider).toBe("cursor");
    });

    it("does not trust a cached answer whose last probe failed", () => {
      probed({ claude: "missing", codex: "failed", cursor: "ready" });
      expect(firstUsableProvider()).toBe("cursor");
    });

    it("skips agents turned off in Settings → Providers", () => {
      probed({ claude: "ready", codex: "ready" });
      choose({ "providers.disabled": "claude" });
      expect(resolveNewChatModelDefaults().provider).toBe("codex");
    });

    it("never overrides a saved default or a connected ChatGPT plan", () => {
      probed({ claude: "missing", opencode: "ready" });
      choose({ "chat.default_provider": "claude" });
      expect(isAutomaticNewChatProvider()).toBe(false);
      expect(resolveNewChatModelDefaults().provider).toBe("claude");
      choose({});
      expect(isAutomaticNewChatProvider()).toBe(true);
      useChatGptStore.setState({ status: {
        phase: "connected", attemptId: null, email: null, error: null,
        profiles: [], activeProfileId: "plan", welcomePending: false, installed: true,
      } });
      expect(resolveNewChatModelDefaults().provider).toBe("codex");
    });
  });
});
