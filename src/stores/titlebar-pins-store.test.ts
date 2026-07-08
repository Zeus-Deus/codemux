/// <reference types="@testing-library/jest-dom/vitest" />
//
// Titlebar pins store — the opt-in set of preset ids shown as tiles in the
// GUI-chrome title bar. Default must be empty (issue: the old
// PinnedPresetTiles logic rendered every chat_agent preset + every
// `pinned` cli preset, flooding the bar since almost all built-ins ship
// `pinned: true` in src-tauri/src/presets.rs for the *legacy PresetBar*,
// an unrelated concept).

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useTitlebarPinsStore } from "./titlebar-pins-store";

const STORAGE_KEY = "codemux:titlebar-pins:v1";

beforeEach(() => {
  localStorage.clear();
  // `persist` rehydrates lazily on first `getState`, so clearing
  // localStorage alone can leave a stale in-memory value once the store
  // has already booted in this test run.
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

afterEach(() => {
  localStorage.clear();
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

describe("useTitlebarPinsStore — default state", () => {
  it("starts empty", () => {
    expect(useTitlebarPinsStore.getState().pinnedIds).toEqual([]);
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(false);
  });
});

describe("useTitlebarPinsStore — toggle / isTitlebarPinned", () => {
  it("toggle adds when not present", () => {
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-claude");
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(true);
  });

  it("toggle removes when present", () => {
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-codex");
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-codex")).toBe(true);
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-codex");
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-codex")).toBe(false);
  });

  it("pinnedIds stays sorted after every toggle", () => {
    useTitlebarPinsStore.getState().toggleTitlebarPin("z-preset");
    useTitlebarPinsStore.getState().toggleTitlebarPin("a-preset");
    useTitlebarPinsStore.getState().toggleTitlebarPin("m-preset");
    expect(useTitlebarPinsStore.getState().pinnedIds).toEqual([
      "a-preset",
      "m-preset",
      "z-preset",
    ]);
  });

  it("toggling one id never affects another", () => {
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-claude");
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-chat-agent");
    expect(useTitlebarPinsStore.getState().pinnedIds).toHaveLength(2);

    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-claude");
    expect(useTitlebarPinsStore.getState().pinnedIds).toEqual(["builtin-chat-agent"]);
  });
});

describe("useTitlebarPinsStore — persistence", () => {
  it("writes pinnedIds to localStorage under the versioned key", () => {
    useTitlebarPinsStore.getState().toggleTitlebarPin("builtin-claude");
    const raw = localStorage.getItem(STORAGE_KEY);
    expect(raw).not.toBeNull();
    const parsed = JSON.parse(raw!);
    expect(parsed.state.pinnedIds).toEqual(["builtin-claude"]);
  });

  it("rehydrates pinnedIds from localStorage on first read", () => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        state: { pinnedIds: ["builtin-claude", "builtin-chat-agent"] },
        version: 0,
      }),
    );
    void useTitlebarPinsStore.persist.rehydrate();

    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(true);
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-chat-agent")).toBe(true);
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-codex")).toBe(false);
  });

  it("storage payload uses the v1 versioned key", () => {
    // Pinned: bumping the storage key changes the migration story. Any
    // future change must come with a `migrate` step in the `persist`
    // config or users lose their pins silently.
    expect(STORAGE_KEY).toBe("codemux:titlebar-pins:v1");
  });
});
