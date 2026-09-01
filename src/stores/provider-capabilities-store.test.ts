/// <reference types="@testing-library/jest-dom/vitest" />
//
// Step 12 Stage 3 — pin the multi-provider capabilities store
// behaviour. Two surfaces matter:
//
// 1. Each provider has its own `caps` slot + `error` slot, refreshed
//    independently. A failure on one provider must NOT clobber the
//    other slots — that's the regression class the Stage 3 refactor
//    introduced (the previous codebase's `selectCapabilities`
//    ternary silently misrouted OpenCode → Codex).
// 2. `selectCapabilities` / `selectError` are exhaustive switches
//    over `AgentChatProviderKind`; adding a fourth provider in the
//    future has to fail at compile time rather than fall through to
//    a stale slot.
//
// The store talks to the Tauri layer via `listChatProviderCapabilities`
// — mocked here so jsdom never reaches a real `invoke`.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockList = vi.fn();

vi.mock("@/tauri/commands", () => ({
  listChatProviderCapabilities: (...args: unknown[]) => mockList(...args),
}));

import {
  CURSOR_CAPABILITY_REFRESH_MS,
  CURSOR_CAPABILITY_TTL_MS,
  GROK_CAPABILITY_REFRESH_MS,
  selectCapabilities,
  selectError,
  selectModel,
  shouldRefreshCursorCapabilities,
  shouldRefreshGrokCapabilities,
  useProviderCapabilities,
} from "./provider-capabilities-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  ProviderChatCapabilities,
} from "@/tauri/types";

function makeCaps(modelId: string): ProviderChatCapabilities {
  return {
    models: [
      {
        id: modelId,
        label: modelId,
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
      },
    ],
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

function resetStore() {
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
    loaded: false,
  });
  mockList.mockReset();
}

beforeEach(resetStore);
afterEach(resetStore);

describe("provider-capabilities-store", () => {
  it("refresh writes to the right slot", async () => {
    mockList.mockImplementation(async (provider: AgentChatProviderKind) =>
      makeCaps(`model-${provider}`),
    );

    await useProviderCapabilities.getState().refresh("opencode");
    const state = useProviderCapabilities.getState();
    expect(state.opencode?.models[0]?.id).toBe("model-opencode");
    // Cross-slot isolation — claude / codex stay null.
    expect(state.claude).toBeNull();
    expect(state.codex).toBeNull();
    expect(state.opencodeError).toBeNull();
  });

  it("refresh on failure populates the right error slot only", async () => {
    mockList.mockImplementation(async (provider: AgentChatProviderKind) => {
      if (provider === "opencode") throw new Error("opencode_not_installed");
      return makeCaps(`model-${provider}`);
    });

    await useProviderCapabilities.getState().refresh("opencode");
    const state = useProviderCapabilities.getState();
    expect(state.opencode).toBeNull();
    expect(state.opencodeError).toBe("opencode_not_installed");
    // Other providers untouched.
    expect(state.claudeError).toBeNull();
    expect(state.codexError).toBeNull();
  });

  it("refreshAll fires all five providers in parallel", async () => {
    const calls: AgentChatProviderKind[] = [];
    mockList.mockImplementation(async (provider: AgentChatProviderKind) => {
      calls.push(provider);
      return makeCaps(`model-${provider}`);
    });

    await useProviderCapabilities.getState().refreshAll();
    // Order is not guaranteed, but every provider must have been
    // called exactly once.
    expect(calls.sort()).toEqual([
      "claude",
      "codex",
      "cursor",
      "grok",
      "opencode",
    ]);
    const state = useProviderCapabilities.getState();
    expect(state.loaded).toBe(true);
    expect(state.claude).not.toBeNull();
    expect(state.codex).not.toBeNull();
    expect(state.cursor).not.toBeNull();
    expect(state.grok).not.toBeNull();
    expect(state.opencode).not.toBeNull();
  });

  it("refreshAll continues when one provider fails", async () => {
    // Critical Stage 3 invariant: an OpenCode-not-installed failure
    // must not block Claude/Codex from loading. `Promise.all` is
    // safe here because each `refresh()` swallows its own error.
    mockList.mockImplementation(async (provider: AgentChatProviderKind) => {
      if (provider === "opencode") throw new Error("opencode_not_installed");
      return makeCaps(`model-${provider}`);
    });

    await useProviderCapabilities.getState().refreshAll();
    const state = useProviderCapabilities.getState();
    expect(state.loaded).toBe(true);
    expect(state.claude?.models[0]?.id).toBe("model-claude");
    expect(state.codex?.models[0]?.id).toBe("model-codex");
    expect(state.cursor?.models[0]?.id).toBe("model-cursor");
    expect(state.grok?.models[0]?.id).toBe("model-grok");
    expect(state.opencode).toBeNull();
    expect(state.opencodeError).toBe("opencode_not_installed");
  });

  it("selectCapabilities returns the matching provider's slot", () => {
    const caps = useProviderCapabilities.getState();
    const claudeCaps = makeCaps("claude-opus-4-7");
    const codexCaps = makeCaps("gpt-5.4");
    const cursorCaps = makeCaps("auto");
    const grokCaps = makeCaps("default");
    const opencodeCaps = makeCaps("openai/gpt-5");
    useProviderCapabilities.setState({
      claude: claudeCaps,
      codex: codexCaps,
      cursor: cursorCaps,
      grok: grokCaps,
      opencode: opencodeCaps,
    });

    const updated = useProviderCapabilities.getState();
    expect(selectCapabilities(updated, "claude")).toBe(claudeCaps);
    expect(selectCapabilities(updated, "codex")).toBe(codexCaps);
    expect(selectCapabilities(updated, "cursor")).toBe(cursorCaps);
    expect(selectCapabilities(updated, "grok")).toBe(grokCaps);
    // The Stage 2 bug: a non-exhaustive ternary returned `state.codex`
    // for any non-claude provider. Pin the fix here so a regression
    // can't reintroduce it silently.
    expect(selectCapabilities(updated, "opencode")).toBe(opencodeCaps);
    expect(selectCapabilities(updated, "opencode")).not.toBe(codexCaps);

    // Smoke: caps.opencode used (not unused-import warning material).
    expect(caps).toBeDefined();
  });

  it("selectError routes to the matching provider's error slot", () => {
    useProviderCapabilities.setState({
      claudeError: "claude broke",
      codexError: "codex broke",
      cursorError: "cursor_not_authenticated",
      grokError: "grok_not_authenticated",
      opencodeError: "opencode_not_installed",
    });
    const state = useProviderCapabilities.getState();
    expect(selectError(state, "claude")).toBe("claude broke");
    expect(selectError(state, "codex")).toBe("codex broke");
    expect(selectError(state, "cursor")).toBe("cursor_not_authenticated");
    expect(selectError(state, "grok")).toBe("grok_not_authenticated");
    expect(selectError(state, "opencode")).toBe("opencode_not_installed");
  });

  it("selectModel finds models by id within capabilities", () => {
    const caps = makeCaps("openai/gpt-5");
    expect(selectModel(caps, "openai/gpt-5")?.id).toBe("openai/gpt-5");
    expect(selectModel(caps, "missing")).toBeNull();
    expect(selectModel(null, "openai/gpt-5")).toBeNull();
    expect(selectModel(caps, null)).toBeNull();
  });

  it("selectModel resolves a dangling 'default' id to models[0]", () => {
    // Persisted drafts/threads from before the backend folded the
    // `"default"` alias row out of the roster still carry that id.
    // When the alias is absent, models[0] is the concrete model it
    // resolved to — selectModel must fall back to it rather than
    // returning null and stranding the UI on a raw "default" string.
    const caps = makeCaps("claude-opus-4-8");
    expect(selectModel(caps, "default")?.id).toBe("claude-opus-4-8");
  });

  it("selectModel still prefers an actual 'default' roster row when present", () => {
    // If the roster DOES contain a "default" row (concrete twin
    // absent, so the backend kept the alias), exact match wins — the
    // fallback only kicks in when the id is dangling.
    const caps: ProviderChatCapabilities = {
      ...makeCaps("claude-opus-4-8"),
      models: [
        ...makeCaps("claude-opus-4-8").models,
        ...makeCaps("default").models,
      ],
    };
    expect(selectModel(caps, "default")?.id).toBe("default");
  });

  it("selectModel does NOT fall back for other unknown ids", () => {
    // The fallback is scoped to the literal historical alias. A
    // genuinely unknown id must stay null so callers can surface it
    // as unresolved instead of silently remapping to models[0].
    const caps = makeCaps("claude-opus-4-8");
    expect(selectModel(caps, "claude-future-9000")).toBeNull();
  });

  it("selectModel('default') on an empty roster returns null", () => {
    const caps: ProviderChatCapabilities = { ...makeCaps("x"), models: [] };
    expect(selectModel(caps, "default")).toBeNull();
  });

  it("OpenCode model has sub_provider populated when injected", () => {
    // Pin the wire-shape contract: OpenCode entries must round-trip
    // `sub_provider` through the store untouched. The store doesn't
    // transform the field, but the contract is shared with the
    // picker's grouping logic.
    const opencodeCaps: ProviderChatCapabilities = {
      ...makeCaps("openai/gpt-5"),
      models: [
        {
          ...makeOpenCodeModel("openai/gpt-5", "openai"),
        },
      ],
    };
    useProviderCapabilities.setState({ opencode: opencodeCaps });
    const state = useProviderCapabilities.getState();
    const caps = selectCapabilities(state, "opencode");
    expect(caps?.models[0]?.sub_provider).toBe("openai");
  });
});

describe("cursor capability polling", () => {
  it("polls past the Rust TTL so every tick is a real refresh", () => {
    // Polling exactly at the TTL makes roughly every other tick land
    // inside the still-valid server cache, silently doubling the
    // effective refresh period the comment above the constant promises.
    expect(CURSOR_CAPABILITY_REFRESH_MS).toBeGreaterThan(
      CURSOR_CAPABILITY_TTL_MS,
    );
  });

  it("skips the poll only when Cursor is known to be missing", () => {
    const report = (installed: boolean) => ({
      provider: "cursor" as const,
      status: "ready" as const,
      installed,
      message: null,
      version: null,
    });
    expect(shouldRefreshCursorCapabilities(report(false))).toBe(false);
    expect(shouldRefreshCursorCapabilities(report(true))).toBe(true);
    // Never probed yet — the harvest is how the picker learns Cursor
    // exists, so an unknown slot must still refresh.
    expect(shouldRefreshCursorCapabilities(null)).toBe(true);
  });
});

describe("grok capability polling", () => {
  it("uses the bounded live-discovery cadence", () => {
    expect(GROK_CAPABILITY_REFRESH_MS).toBe(CURSOR_CAPABILITY_REFRESH_MS);
  });

  it("skips the poll only when Grok is known to be missing", () => {
    const report = (installed: boolean) => ({
      provider: "grok" as const,
      status: "ready" as const,
      installed,
      message: null,
      version: null,
    });
    expect(shouldRefreshGrokCapabilities(report(false))).toBe(false);
    expect(shouldRefreshGrokCapabilities(report(true))).toBe(true);
    expect(shouldRefreshGrokCapabilities(null)).toBe(true);
  });
});

function makeOpenCodeModel(id: string, subProvider: string): ChatModelInfo {
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
    sub_provider: subProvider,
    is_free: false,
  };
}
