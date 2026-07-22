import { describe, it, expect, vi, beforeEach } from "vitest";

const mockDbGetUiState = vi.fn();
const mockDbSetUiState = vi.fn();

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: (...args: unknown[]) => mockDbGetUiState(...args),
  dbSetUiState: (...args: unknown[]) => mockDbSetUiState(...args),
}));

import {
  useSidebarInboxStore,
  SETTLED_UI_STATE_KEY,
  __resetSidebarInboxStoreForTests,
} from "./sidebar-inbox-store";

describe("sidebar-inbox-store", () => {
  beforeEach(() => {
    __resetSidebarInboxStoreForTests();
    vi.clearAllMocks();
    mockDbGetUiState.mockResolvedValue(null);
    mockDbSetUiState.mockResolvedValue(undefined);
  });

  it("load() reads the persisted settled list", async () => {
    mockDbGetUiState.mockResolvedValue(
      JSON.stringify([{ id: "ws-2", at: 1000 }, { id: "ws-1", at: 500 }]),
    );
    await useSidebarInboxStore.getState().load();
    expect(mockDbGetUiState).toHaveBeenCalledWith(SETTLED_UI_STATE_KEY);
    expect(useSidebarInboxStore.getState().loaded).toBe(true);
    expect(useSidebarInboxStore.getState().settled).toEqual([
      { id: "ws-2", at: 1000 },
      { id: "ws-1", at: 500 },
    ]);
  });

  it("load() tolerates corrupt persisted JSON", async () => {
    mockDbGetUiState.mockResolvedValue("{not json");
    await useSidebarInboxStore.getState().load();
    expect(useSidebarInboxStore.getState().loaded).toBe(true);
    expect(useSidebarInboxStore.getState().settled).toEqual([]);
  });

  it("load() drops malformed entries", async () => {
    mockDbGetUiState.mockResolvedValue(
      JSON.stringify([{ id: "ok", at: 1 }, { id: 42 }, "nope", null]),
    );
    await useSidebarInboxStore.getState().load();
    expect(useSidebarInboxStore.getState().settled).toEqual([
      { id: "ok", at: 1 },
    ]);
  });

  it("settle() prepends (newest first), persists, and is idempotent", () => {
    const store = useSidebarInboxStore.getState();
    store.settle("ws-1");
    useSidebarInboxStore.getState().settle("ws-2");
    useSidebarInboxStore.getState().settle("ws-1"); // duplicate — no-op

    const settled = useSidebarInboxStore.getState().settled;
    expect(settled.map((e) => e.id)).toEqual(["ws-2", "ws-1"]);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(2);
    const [key, value] = mockDbSetUiState.mock.calls[1] as [string, string];
    expect(key).toBe(SETTLED_UI_STATE_KEY);
    expect(
      (JSON.parse(value) as { id: string }[]).map((e) => e.id),
    ).toEqual(["ws-2", "ws-1"]);
  });

  it("unsettle() removes the entry and persists", () => {
    useSidebarInboxStore.getState().settle("ws-1");
    useSidebarInboxStore.getState().settle("ws-2");
    mockDbSetUiState.mockClear();

    useSidebarInboxStore.getState().unsettle("ws-1");
    expect(
      useSidebarInboxStore.getState().settled.map((e) => e.id),
    ).toEqual(["ws-2"]);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);

    // Unknown id — no state change, no persist.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore.getState().unsettle("ws-404");
    expect(mockDbSetUiState).not.toHaveBeenCalled();
  });

  it("prune() drops ids whose workspace vanished, persisting only on change", () => {
    useSidebarInboxStore.getState().settle("ws-1");
    useSidebarInboxStore.getState().settle("ws-gone");
    mockDbSetUiState.mockClear();

    useSidebarInboxStore.getState().prune(new Set(["ws-1", "ws-other"]));
    expect(
      useSidebarInboxStore.getState().settled.map((e) => e.id),
    ).toEqual(["ws-1"]);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);

    // Nothing to prune → no persist.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore.getState().prune(new Set(["ws-1"]));
    expect(mockDbSetUiState).not.toHaveBeenCalled();
  });

  it("setFilter() is session-only (never persisted)", () => {
    useSidebarInboxStore.getState().setFilter("/home/u/projects/app");
    expect(useSidebarInboxStore.getState().filter).toBe(
      "/home/u/projects/app",
    );
    expect(mockDbSetUiState).not.toHaveBeenCalled();
  });
});
