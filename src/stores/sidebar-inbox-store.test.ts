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
    const blob = JSON.parse(value) as {
      settled: { id: string }[];
      keepActive: string[];
      activity: Record<string, number>;
    };
    expect(blob.settled.map((e) => e.id)).toEqual(["ws-2", "ws-1"]);
    expect(blob.keepActive).toEqual([]);
    expect(blob.activity).toEqual({});
  });

  it("unsettle() removes the entry and persists", () => {
    useSidebarInboxStore.getState().settle("ws-1");
    useSidebarInboxStore.getState().settle("ws-2");
    mockDbSetUiState.mockClear();

    useSidebarInboxStore.getState().unsettle("ws-1", "activity");
    expect(
      useSidebarInboxStore.getState().settled.map((e) => e.id),
    ).toEqual(["ws-2"]);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);

    // Unknown id, no pin to clear — no state change, no persist.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore.getState().unsettle("ws-404", "activity");
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

  it("load() migrates the legacy bare-array shape (keepActive/activity empty)", async () => {
    mockDbGetUiState.mockResolvedValue(
      JSON.stringify([{ id: "ws-1", at: 500 }]),
    );
    await useSidebarInboxStore.getState().load();
    const s = useSidebarInboxStore.getState();
    expect(s.settled).toEqual([{ id: "ws-1", at: 500 }]);
    expect(s.keepActive).toEqual({});
    expect(s.activity).toEqual({});
  });

  it("load() round-trips the new object shape", async () => {
    mockDbGetUiState.mockResolvedValue(
      JSON.stringify({
        settled: [{ id: "ws-1", at: 500 }],
        keepActive: ["ws-2"],
        activity: { "ws-3": 12345 },
      }),
    );
    await useSidebarInboxStore.getState().load();
    const s = useSidebarInboxStore.getState();
    expect(s.settled).toEqual([{ id: "ws-1", at: 500 }]);
    expect(s.keepActive).toEqual({ "ws-2": true });
    expect(s.activity).toEqual({ "ws-3": 12345 });
  });

  it("unsettle('user') pins keep-active; settle() and unsettle('activity') clear it", () => {
    const store = useSidebarInboxStore.getState();
    store.settle("ws-1");
    // User un-settles → keep-active pin set, entry removed.
    useSidebarInboxStore.getState().unsettle("ws-1", "user");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    expect(useSidebarInboxStore.getState().settled).toEqual([]);

    // Explicit settle ends the pin.
    useSidebarInboxStore.getState().settle("ws-1");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({});
    expect(
      useSidebarInboxStore.getState().settled.map((e) => e.id),
    ).toEqual(["ws-1"]);

    // Pin again, then an activity un-settle clears it.
    useSidebarInboxStore.getState().unsettle("ws-1", "user");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    useSidebarInboxStore.getState().unsettle("ws-1", "activity");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({});
  });

  it("noteActivity() stamps, clears any pin, and throttles rapid re-stamps", () => {
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_000_000);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);

    // A second stamp within 60s does not move the value or persist.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_030_000);
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_000_000);
    expect(mockDbSetUiState).not.toHaveBeenCalled();

    // Past the throttle window it advances and persists.
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_100_000);
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_100_000);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
  });

  it("noteActivity() clears a pin even when the stamp itself is throttled", () => {
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
    useSidebarInboxStore.getState().unsettle("ws-1", "user");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    mockDbSetUiState.mockClear();

    // Throttled stamp (within 60s) but the pin still clears + persists.
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_010_000);
    expect(useSidebarInboxStore.getState().keepActive).toEqual({});
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_000_000);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
  });

  it("prune() also drops keep-active pins and activity stamps", () => {
    useSidebarInboxStore.getState().settle("ws-1");
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
    // Pin a doomed workspace last so noteActivity can't clear the pin.
    useSidebarInboxStore.getState().unsettle("ws-gone", "user"); // pin only
    mockDbSetUiState.mockClear();

    useSidebarInboxStore.getState().prune(new Set(["ws-1"]));
    const s = useSidebarInboxStore.getState();
    expect(s.settled.map((e) => e.id)).toEqual(["ws-1"]);
    expect(s.keepActive).toEqual({});
    expect(s.activity).toEqual({ "ws-1": 1_000_000 });
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);

    // Nothing left to prune → no persist.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore.getState().prune(new Set(["ws-1"]));
    expect(mockDbSetUiState).not.toHaveBeenCalled();
  });
});
