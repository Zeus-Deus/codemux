/// <reference types="@testing-library/jest-dom/vitest" />
//
// Step 12 Stage 6 — pin the picker favorites store contract.
//
// Three concerns covered here:
//
// 1. Toggle / isFavorite / getKey work and round-trip through the
//    `${provider}::${modelId}` key shape.
// 2. Cross-provider keys never collide — favoriting `claude::sonnet`
//    must be independent of favoriting `opencode::sonnet` (a real
//    case once OpenCode federates Anthropic alongside the Claude
//    SDK route).
// 3. The store survives a localStorage round-trip via the `persist`
//    middleware. We exercise this by writing through one store
//    instance, re-creating the store, and verifying the new instance
//    rehydrates the same favorites array.

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  pickerFavoriteKey,
  usePickerFavorites,
} from "./picker-favorites-store";

const STORAGE_KEY = "codemux:picker-favorites:v1";

beforeEach(() => {
  localStorage.clear();
  // Reset the in-memory zustand state too — `persist` rehydrates
  // from localStorage on first access of `getState`, so clearing
  // localStorage alone leaves a stale `[]` in memory if the store
  // already booted.
  usePickerFavorites.setState({ favorites: [] });
});

afterEach(() => {
  localStorage.clear();
  usePickerFavorites.setState({ favorites: [] });
});

describe("usePickerFavorites — toggle / isFavorite", () => {
  it("toggle adds when not present", () => {
    expect(usePickerFavorites.getState().isFavorite("claude", "claude-opus-4-7")).toBe(false);
    usePickerFavorites.getState().toggle("claude", "claude-opus-4-7");
    expect(usePickerFavorites.getState().isFavorite("claude", "claude-opus-4-7")).toBe(true);
  });

  it("toggle removes when present", () => {
    usePickerFavorites.getState().toggle("codex", "gpt-5.4");
    expect(usePickerFavorites.getState().isFavorite("codex", "gpt-5.4")).toBe(true);
    usePickerFavorites.getState().toggle("codex", "gpt-5.4");
    expect(usePickerFavorites.getState().isFavorite("codex", "gpt-5.4")).toBe(false);
  });

  it("getKey matches the exported helper format", () => {
    expect(usePickerFavorites.getState().getKey("claude", "x")).toBe("claude::x");
    expect(pickerFavoriteKey("opencode", "openai/gpt-5")).toBe(
      "opencode::openai/gpt-5",
    );
  });

  it("favorites array stays sorted after every toggle", () => {
    // Pinned: a sorted array means the persisted JSON payload is
    // deterministic across reloads — important so a refresh doesn't
    // appear to reorder favorites for the user.
    usePickerFavorites.getState().toggle("opencode", "openai/gpt-5");
    usePickerFavorites.getState().toggle("claude", "claude-opus-4-7");
    usePickerFavorites.getState().toggle("codex", "gpt-5.4");
    expect(usePickerFavorites.getState().favorites).toEqual([
      "claude::claude-opus-4-7",
      "codex::gpt-5.4",
      "opencode::openai/gpt-5",
    ]);
  });
});

describe("usePickerFavorites — cross-provider isolation", () => {
  it("same model id across providers gets independent keys", () => {
    // The "anthropic via Claude SDK" vs "anthropic via OpenCode"
    // case from the Stage 6 spec. Each route is its own provider, so
    // each is its own favorite.
    const sonnetSlug = "claude-sonnet-4-6";
    usePickerFavorites.getState().toggle("claude", sonnetSlug);
    expect(
      usePickerFavorites.getState().isFavorite("claude", sonnetSlug),
    ).toBe(true);
    expect(
      usePickerFavorites.getState().isFavorite("opencode", sonnetSlug),
    ).toBe(false);

    usePickerFavorites
      .getState()
      .toggle("opencode", `anthropic/${sonnetSlug}`);
    expect(
      usePickerFavorites.getState().isFavorite("claude", sonnetSlug),
    ).toBe(true);
    expect(
      usePickerFavorites
        .getState()
        .isFavorite("opencode", `anthropic/${sonnetSlug}`),
    ).toBe(true);
  });

  it("toggling one provider's entry never affects another's", () => {
    usePickerFavorites.getState().toggle("claude", "haiku");
    usePickerFavorites.getState().toggle("opencode", "openrouter/x-ai/grok-2");
    expect(usePickerFavorites.getState().favorites).toHaveLength(2);

    usePickerFavorites.getState().toggle("claude", "haiku");
    expect(usePickerFavorites.getState().favorites).toEqual([
      "opencode::openrouter/x-ai/grok-2",
    ]);
  });
});

describe("usePickerFavorites — persistence", () => {
  it("writes favorites to localStorage under the versioned key", () => {
    usePickerFavorites.getState().toggle("claude", "claude-opus-4-7");
    const raw = localStorage.getItem(STORAGE_KEY);
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw!);
    // zustand `persist` wraps state in `{ state, version }`. We just
    // care that our key is in there.
    expect(parsed.state.favorites).toEqual(["claude::claude-opus-4-7"]);
  });

  it("rehydrates favorites from localStorage on first read", () => {
    // Simulate a prior session writing a favorites payload.
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        state: {
          favorites: ["claude::claude-opus-4-7", "opencode::openai/gpt-5"],
        },
        version: 0,
      }),
    );
    // Force the store to re-hydrate from storage. (zustand persist
    // hydrates lazily; calling `rehydrate()` is the explicit form
    // exposed for tests.)
    void usePickerFavorites.persist.rehydrate();

    expect(
      usePickerFavorites.getState().isFavorite("claude", "claude-opus-4-7"),
    ).toBe(true);
    expect(
      usePickerFavorites.getState().isFavorite("opencode", "openai/gpt-5"),
    ).toBe(true);
    expect(
      usePickerFavorites.getState().isFavorite("codex", "gpt-5.4"),
    ).toBe(false);
  });

  it("storage payload uses the v1 versioned key", () => {
    // Pinned: bumping the storage key changes the migration story.
    // Any future change must come with a `migrate` step in the
    // `persist` config or users lose their favorites silently.
    expect(STORAGE_KEY).toBe("codemux:picker-favorites:v1");
  });
});
