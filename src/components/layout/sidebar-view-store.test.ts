import { beforeEach, describe, expect, it, vi } from "vitest";

const stored = new Map<string, string>();
vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn((key: string) => Promise.resolve(stored.get(key) ?? null)),
  dbSetUiState: vi.fn((key: string, value: string) => {
    stored.set(key, value);
    return Promise.resolve();
  }),
}));

import { dbSetUiState } from "@/tauri/commands";
import {
  SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY,
  SIDEBAR_VIEW_UI_STATE_KEY,
  __resetSidebarViewStoreForTests,
  parseSidebarView,
  useSidebarViewStore,
} from "./sidebar-view-store";

beforeEach(() => {
  stored.clear();
  vi.clearAllMocks();
  __resetSidebarViewStoreForTests();
});

describe("sidebar view store", () => {
  it("defaults to Activity when nothing is stored", async () => {
    await useSidebarViewStore.getState().load();
    expect(useSidebarViewStore.getState()).toMatchObject({
      loaded: true,
      view: "activity",
      folded: {},
    });
  });

  it("restores a persisted Projects choice and folded groups", async () => {
    stored.set(SIDEBAR_VIEW_UI_STATE_KEY, "projects");
    stored.set(SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY, JSON.stringify(["/p/a"]));
    await useSidebarViewStore.getState().load();
    expect(useSidebarViewStore.getState().view).toBe("projects");
    expect(useSidebarViewStore.getState().folded).toEqual({ "/p/a": true });
  });

  it("falls back to Activity for unknown or corrupt values", async () => {
    expect(parseSidebarView("timeline")).toBe("activity");
    expect(parseSidebarView(null)).toBe("activity");
    stored.set(SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY, "{not json");
    await useSidebarViewStore.getState().load();
    expect(useSidebarViewStore.getState().folded).toEqual({});
  });

  it("persists the choice when toggled", async () => {
    await useSidebarViewStore.getState().load();
    useSidebarViewStore.getState().toggleView();
    expect(useSidebarViewStore.getState().view).toBe("projects");
    expect(dbSetUiState).toHaveBeenLastCalledWith(SIDEBAR_VIEW_UI_STATE_KEY, "projects");
    useSidebarViewStore.getState().toggleView();
    expect(stored.get(SIDEBAR_VIEW_UI_STATE_KEY)).toBe("activity");

    // A fresh session reads the last choice back.
    __resetSidebarViewStoreForTests();
    stored.set(SIDEBAR_VIEW_UI_STATE_KEY, "projects");
    await useSidebarViewStore.getState().load();
    expect(useSidebarViewStore.getState().view).toBe("projects");
  });

  it("does not let a late read override a choice made before it resolved", async () => {
    stored.set(SIDEBAR_VIEW_UI_STATE_KEY, "activity");
    const pending = useSidebarViewStore.getState().load();
    useSidebarViewStore.getState().setView("projects");
    await pending;
    expect(useSidebarViewStore.getState().view).toBe("projects");
  });

  it("persists fold state per project", async () => {
    await useSidebarViewStore.getState().load();
    useSidebarViewStore.getState().setFolded("/p/a", true);
    useSidebarViewStore.getState().setFolded("/p/b", true);
    useSidebarViewStore.getState().setFolded("/p/a", false);
    expect(useSidebarViewStore.getState().folded).toEqual({ "/p/b": true });
    expect(JSON.parse(stored.get(SIDEBAR_FOLDED_PROJECTS_UI_STATE_KEY)!)).toEqual(["/p/b"]);
  });
});
