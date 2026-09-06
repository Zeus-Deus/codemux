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
const mockHealth = vi.fn();

vi.mock("@/tauri/commands", () => ({
  listChatProviderCapabilities: (...args: unknown[]) => mockList(...args),
  agentChatProviderHealth: (...args: unknown[]) => mockHealth(...args),
}));

import {
  _resetProviderCapabilityIntentForTests,
  CURSOR_CAPABILITY_REFRESH_MS,
  CURSOR_CAPABILITY_TTL_MS,
  GROK_CAPABILITY_REFRESH_MS,
  PROVIDER_CAPABILITIES_STORAGE_KEY,
  refreshProviderCapabilitiesForIntent,
  resetProviderCapabilities,
  selectCapabilities,
  selectError,
  selectModel,
  selectProviderCapabilitiesLoaded,
  shouldRefreshCursorCapabilities,
  shouldRefreshGrokCapabilities,
  useProviderCapabilities,
} from "./provider-capabilities-store";
import {
  emptyHealthSlot,
  useProviderHealth,
} from "./provider-health-store";
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
  _resetProviderCapabilityIntentForTests();
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
    loadedProviders: {},
  });
  localStorage.removeItem(PROVIDER_CAPABILITIES_STORAGE_KEY);
  mockList.mockReset();
  mockHealth.mockReset();
  mockHealth.mockImplementation(async (provider: AgentChatProviderKind) => ({
    provider,
    status: "ready",
    installed: true,
    message: null,
    version: null,
  }));
  useProviderHealth.setState({
    slots: {
      claude: emptyHealthSlot(),
      codex: emptyHealthSlot(),
      cursor: emptyHealthSlot(),
      grok: emptyHealthSlot(),
      opencode: emptyHealthSlot(),
    },
  });
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

  it("single-flights concurrent refreshes for one provider", async () => {
    let resolve!: (caps: ProviderChatCapabilities) => void;
    mockList.mockReturnValue(
      new Promise<ProviderChatCapabilities>((done) => {
        resolve = done;
      }),
    );

    const first = useProviderCapabilities.getState().refresh("claude");
    const second = useProviderCapabilities.getState().refresh("claude");

    expect(first).toBe(second);
    expect(mockList).toHaveBeenCalledTimes(1);
    resolve(makeCaps("claude-opus"));
    await Promise.all([first, second]);
  });

  it("shows cached capabilities then refreshes them once on first intent", async () => {
    useProviderCapabilities.setState({ claude: makeCaps("claude-cached") });
    mockList.mockResolvedValue(makeCaps("claude-live"));

    await refreshProviderCapabilitiesForIntent("claude");
    await refreshProviderCapabilitiesForIntent("claude");

    expect(mockList).toHaveBeenCalledTimes(1);
    expect(mockHealth).toHaveBeenCalledTimes(1);
    expect(useProviderCapabilities.getState().claude?.models[0]?.id).toBe(
      "claude-live",
    );
  });

  it("retries a transient capability failure on the next explicit intent", async () => {
    mockList
      .mockRejectedValueOnce(new Error("temporary provider failure"))
      .mockResolvedValueOnce(makeCaps("claude-recovered"));

    await refreshProviderCapabilitiesForIntent("claude");
    expect(useProviderCapabilities.getState().claudeError).toBe(
      "temporary provider failure",
    );

    await refreshProviderCapabilitiesForIntent("claude");

    expect(mockList).toHaveBeenCalledTimes(2);
    expect(useProviderCapabilities.getState().claudeError).toBeNull();
    expect(useProviderCapabilities.getState().claude?.models[0]?.id).toBe(
      "claude-recovered",
    );
  });

  it("reharvests Cursor on a later intent after its refresh cadence", async () => {
    let now = 10_000;
    const nowSpy = vi.spyOn(Date, "now").mockImplementation(() => now);
    mockList
      .mockResolvedValueOnce(makeCaps("cursor-cached-refresh"))
      .mockResolvedValueOnce(makeCaps("cursor-live-refresh"));

    try {
      await refreshProviderCapabilitiesForIntent("cursor");
      await refreshProviderCapabilitiesForIntent("cursor");
      expect(mockList).toHaveBeenCalledTimes(1);

      now += CURSOR_CAPABILITY_REFRESH_MS;
      await refreshProviderCapabilitiesForIntent("cursor");

      expect(mockList).toHaveBeenCalledTimes(2);
      expect(useProviderCapabilities.getState().cursor?.models[0]?.id).toBe(
        "cursor-live-refresh",
      );
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("persists only last-known capability metadata", () => {
    const claude = makeCaps("claude-cached");
    useProviderCapabilities.setState({
      claude,
      claudeError: "transient failure",
    });

    const persisted = JSON.parse(
      localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY) ?? "null",
    ) as { state?: Record<string, unknown> } | null;
    expect(persisted?.state).toEqual({
      claude,
      codex: null,
      cursor: null,
      grok: null,
      opencode: null,
    });
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
    expect(selectProviderCapabilitiesLoaded(state, "opencode")).toBe(true);
  });

  it("marks a successful empty catalog loaded for that provider", async () => {
    mockList.mockResolvedValue({ ...makeCaps("unused"), models: [] });

    await useProviderCapabilities.getState().refresh("claude");

    const state = useProviderCapabilities.getState();
    expect(state.claude?.models).toEqual([]);
    expect(selectProviderCapabilitiesLoaded(state, "claude")).toBe(true);
    expect(selectProviderCapabilitiesLoaded(state, "codex")).toBe(false);
  });

  it("parallel per-provider refreshes settle every slot and derive loaded", async () => {
    const calls: AgentChatProviderKind[] = [];
    mockList.mockImplementation(async (provider: AgentChatProviderKind) => {
      calls.push(provider);
      return makeCaps(`model-${provider}`);
    });

    const store = useProviderCapabilities.getState();
    await Promise.all([
      store.refresh("claude"),
      store.refresh("codex"),
      store.refresh("cursor"),
      store.refresh("grok"),
      store.refresh("opencode"),
    ]);
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
    expect(state.claude).not.toBeNull();
    expect(state.codex).not.toBeNull();
    expect(state.cursor).not.toBeNull();
    expect(state.grok).not.toBeNull();
    expect(state.opencode).not.toBeNull();
  });

  it("one provider's failure does not block the others from settling", async () => {
    // Critical Stage 3 invariant: an OpenCode-not-installed failure
    // must not block Claude/Codex from loading. `Promise.all` is
    // safe here because each `refresh()` swallows its own error.
    mockList.mockImplementation(async (provider: AgentChatProviderKind) => {
      if (provider === "opencode") throw new Error("opencode_not_installed");
      return makeCaps(`model-${provider}`);
    });

    const store = useProviderCapabilities.getState();
    await Promise.all([
      store.refresh("claude"),
      store.refresh("codex"),
      store.refresh("cursor"),
      store.refresh("grok"),
      store.refresh("opencode"),
    ]);
    const state = useProviderCapabilities.getState();
    expect(state.claude?.models[0]?.id).toBe("model-claude");
    expect(state.codex?.models[0]?.id).toBe("model-codex");
    expect(state.cursor?.models[0]?.id).toBe("model-cursor");
    expect(state.grok?.models[0]?.id).toBe("model-grok");
    expect(state.opencode).toBeNull();
    expect(state.opencodeError).toBe("opencode_not_installed");
  });

  it("resetProviderCapabilities clears memory and the persisted catalog", () => {
    useProviderCapabilities.setState({
      claude: makeCaps("claude-cached"),
      claudeError: "stale error",
      loadedProviders: { claude: true },
    });
    expect(
      localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY),
    ).not.toBeNull();

    resetProviderCapabilities();

    const state = useProviderCapabilities.getState();
    expect(state.claude).toBeNull();
    expect(state.claudeError).toBeNull();
    expect(state.loadedProviders).toEqual({});
    expect(localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY)).toBeNull();
  });

  it("discards a refresh that resolves after resetProviderCapabilities", async () => {
    // A model-picker harvest can spend seconds in a CLI. If the user signs
    // out mid-flight, the late result must not be written back — neither to
    // memory nor (via the persist middleware) to localStorage, which reset
    // just cleared.
    let resolveHarvest!: (caps: ProviderChatCapabilities) => void;
    mockList.mockReturnValue(
      new Promise<ProviderChatCapabilities>((done) => {
        resolveHarvest = done;
      }),
    );

    const flight = useProviderCapabilities.getState().refresh("claude");
    resetProviderCapabilities();
    resolveHarvest(makeCaps("claude-stale"));
    await flight;

    const state = useProviderCapabilities.getState();
    expect(state.claude).toBeNull();
    expect(state.claudeError).toBeNull();
    // The doomed flight must not mark its slot settled either.
    expect(state.loadedProviders).toEqual({});
    expect(localStorage.getItem(PROVIDER_CAPABILITIES_STORAGE_KEY)).toBeNull();
  });

  it("discards a refresh that fails after resetProviderCapabilities", async () => {
    let rejectHarvest!: (err: Error) => void;
    mockList.mockReturnValue(
      new Promise<ProviderChatCapabilities>((_done, fail) => {
        rejectHarvest = fail;
      }),
    );

    const flight = useProviderCapabilities.getState().refresh("claude");
    resetProviderCapabilities();
    rejectHarvest(new Error("late harvest failure"));
    await flight;

    const state = useProviderCapabilities.getState();
    expect(state.claudeError).toBeNull();
    expect(state.claude).toBeNull();
    expect(state.loadedProviders).toEqual({});
  });

  it("a refresh started after reset is not deduped against the doomed flight", async () => {
    let resolveStale!: (caps: ProviderChatCapabilities) => void;
    mockList
      .mockReturnValueOnce(
        new Promise<ProviderChatCapabilities>((done) => {
          resolveStale = done;
        }),
      )
      .mockResolvedValueOnce(makeCaps("claude-fresh"));

    const stale = useProviderCapabilities.getState().refresh("claude");
    resetProviderCapabilities();
    const fresh = useProviderCapabilities.getState().refresh("claude");

    // The post-reset (post-sign-in) refresh must be a new flight with a new
    // invoke, not a join on the pre-reset one.
    expect(fresh).not.toBe(stale);
    expect(mockList).toHaveBeenCalledTimes(2);

    await fresh;
    resolveStale(makeCaps("claude-stale"));
    await stale;

    const state = useProviderCapabilities.getState();
    // The stale flight settles last but must not clobber the fresh result
    // or the fresh flight's lifecycle bookkeeping.
    expect(state.claude?.models[0]?.id).toBe("claude-fresh");
    expect(selectProviderCapabilitiesLoaded(state, "claude")).toBe(true);
  });

  it("resetProviderCapabilities forgets completed intents so the next intent re-harvests", async () => {
    mockList.mockResolvedValue(makeCaps("claude-live"));

    await refreshProviderCapabilitiesForIntent("claude");
    expect(mockList).toHaveBeenCalledTimes(1);

    resetProviderCapabilities();
    await refreshProviderCapabilitiesForIntent("claude");

    expect(mockList).toHaveBeenCalledTimes(2);
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

  it("skips a re-harvest only when Cursor is known to be missing", () => {
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

  it("skips a re-harvest only when Grok is known to be missing", () => {
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
