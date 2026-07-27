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
  resolveSettledTimestamp,
  __resetSidebarInboxStoreForTests,
} from "./sidebar-inbox-store";

/** The persisted blob shape, as the tests read it back out of the mock. */
interface PersistedBlob {
  settled: { id: string; at: number; workEndedAt?: number }[];
  snoozed: { id: string; at: number; until: number }[];
  keepActive: string[];
  activity: Record<string, number>;
}

function lastPersistedBlob(): PersistedBlob {
  const calls = mockDbSetUiState.mock.calls;
  const [key, value] = calls[calls.length - 1] as [string, string];
  expect(key).toBe(SETTLED_UI_STATE_KEY);
  return JSON.parse(value) as PersistedBlob;
}

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

  it("noteActivity() stamps and throttles rapid re-stamps", () => {
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

  it("noteActivity({ clearPin: true }) clears a pin even when the stamp is throttled", () => {
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
    useSidebarInboxStore.getState().unsettle("ws-1", "user");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    mockDbSetUiState.mockClear();

    // Throttled stamp (within 60s) but the pin still clears + persists.
    useSidebarInboxStore
      .getState()
      .noteActivity("ws-1", 1_010_000, { clearPin: true });
    expect(useSidebarInboxStore.getState().keepActive).toEqual({});
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_000_000);
    expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
  });

  it("noteActivity() without clearPin stamps but leaves the keep-active pin", () => {
    useSidebarInboxStore.getState().unsettle("ws-1", "user");
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    mockDbSetUiState.mockClear();

    // Merely selecting a workspace (or a first-seen baseline) is not agent
    // activity: the stamp lands, the pin survives.
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
    expect(useSidebarInboxStore.getState().activity["ws-1"]).toBe(1_000_000);
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });

    // An explicit clearPin: false behaves the same, and a throttled re-stamp
    // stays a full no-op rather than dropping the pin.
    mockDbSetUiState.mockClear();
    useSidebarInboxStore
      .getState()
      .noteActivity("ws-1", 1_010_000, { clearPin: false });
    expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
    expect(mockDbSetUiState).not.toHaveBeenCalled();
  });

  it("prune() also drops keep-active pins and activity stamps", () => {
    useSidebarInboxStore.getState().settle("ws-1");
    useSidebarInboxStore.getState().noteActivity("ws-1", 1_000_000);
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

  describe("settled sort key", () => {
    it("resolveSettledTimestamp() prefers workEndedAt and falls back to at", () => {
      expect(resolveSettledTimestamp({ id: "a", at: 500 })).toBe(500);
      expect(
        resolveSettledTimestamp({ id: "a", at: 500, workEndedAt: 100 }),
      ).toBe(100);
      // A zero work-end time is still a known time, not "unknown".
      expect(
        resolveSettledTimestamp({ id: "a", at: 500, workEndedAt: 0 }),
      ).toBe(0);
    });

    it("settle() stores workEndedAt when given and omits it otherwise", () => {
      useSidebarInboxStore.getState().settle("ws-old", 111);
      useSidebarInboxStore.getState().settle("ws-unknown");

      const settled = useSidebarInboxStore.getState().settled;
      const old = settled.find((e) => e.id === "ws-old");
      const unknown = settled.find((e) => e.id === "ws-unknown");
      expect(old?.workEndedAt).toBe(111);
      expect(unknown?.workEndedAt).toBeUndefined();
      // The unknown row still resolves to something sortable.
      expect(resolveSettledTimestamp(unknown!)).toBe(unknown!.at);

      expect(
        lastPersistedBlob().settled.find((e) => e.id === "ws-old")?.workEndedAt,
      ).toBe(111);
    });
  });

  describe("snooze", () => {
    it("snooze()/unsnooze() round-trips, persisting once per mutation", () => {
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      const entry = useSidebarInboxStore.getState().snoozed[0];
      expect(entry?.id).toBe("ws-1");
      expect(entry?.until).toBe(5_000);
      expect(typeof entry?.at).toBe("number");
      expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
      expect(lastPersistedBlob().snoozed).toEqual([
        { id: "ws-1", at: entry!.at, until: 5_000 },
      ]);

      mockDbSetUiState.mockClear();
      useSidebarInboxStore.getState().unsnooze("ws-1", "timer");
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
      expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
      expect(lastPersistedBlob().snoozed).toEqual([]);

      // Unknown id, no pin to touch — no state change, no persist.
      mockDbSetUiState.mockClear();
      useSidebarInboxStore.getState().unsnooze("ws-404", "timer");
      expect(mockDbSetUiState).not.toHaveBeenCalled();
    });

    it("re-snoozing replaces the entry rather than duplicating it", () => {
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      useSidebarInboxStore.getState().snooze("ws-1", 9_000);
      const snoozed = useSidebarInboxStore.getState().snoozed;
      expect(snoozed.map((e) => e.id)).toEqual(["ws-1"]);
      expect(snoozed[0]?.until).toBe(9_000);
    });

    it("the two shelves are mutually exclusive in both directions", () => {
      useSidebarInboxStore.getState().settle("ws-1");
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      expect(useSidebarInboxStore.getState().settled).toEqual([]);
      expect(
        useSidebarInboxStore.getState().snoozed.map((e) => e.id),
      ).toEqual(["ws-1"]);

      mockDbSetUiState.mockClear();
      useSidebarInboxStore.getState().settle("ws-1");
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
      expect(
        useSidebarInboxStore.getState().settled.map((e) => e.id),
      ).toEqual(["ws-1"]);
      expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
      const blob = lastPersistedBlob();
      expect(blob.snoozed).toEqual([]);
      expect(blob.settled.map((e) => e.id)).toEqual(["ws-1"]);
    });

    it("snooze() clears a keep-active pin", () => {
      useSidebarInboxStore.getState().unsettle("ws-1", "user");
      expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });

      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      expect(useSidebarInboxStore.getState().keepActive).toEqual({});
      expect(lastPersistedBlob().keepActive).toEqual([]);
    });

    it("unsnooze('user') pins keep-active; 'activity' clears it; 'timer' leaves it alone", () => {
      // Explicit wake-now behaves like unsettle("user").
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      useSidebarInboxStore.getState().unsnooze("ws-1", "user");
      expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-1": true });
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);

      // The agent resurfacing is the signal a pin waits for.
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      useSidebarInboxStore.getState().unsnooze("ws-1", "activity");
      expect(useSidebarInboxStore.getState().keepActive).toEqual({});
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);

      // The clock expiring says nothing about the pin either way.
      useSidebarInboxStore.getState().unsettle("ws-2", "user");
      useSidebarInboxStore.getState().snooze("ws-3", 5_000);
      useSidebarInboxStore.getState().unsnooze("ws-3", "timer");
      expect(useSidebarInboxStore.getState().keepActive).toEqual({ "ws-2": true });
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);

      // ...and a timer wake on an unpinned, unsnoozed id is a full no-op.
      mockDbSetUiState.mockClear();
      useSidebarInboxStore.getState().unsnooze("ws-3", "timer");
      expect(mockDbSetUiState).not.toHaveBeenCalled();
    });

    it("prune() drops snoozed entries for vanished workspaces", () => {
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      useSidebarInboxStore.getState().snooze("ws-gone", 5_000);
      mockDbSetUiState.mockClear();

      useSidebarInboxStore.getState().prune(new Set(["ws-1"]));
      expect(
        useSidebarInboxStore.getState().snoozed.map((e) => e.id),
      ).toEqual(["ws-1"]);
      expect(mockDbSetUiState).toHaveBeenCalledTimes(1);
      expect(lastPersistedBlob().snoozed.map((e) => e.id)).toEqual(["ws-1"]);

      // A snooze-only change still counts as a change; nothing left → no persist.
      mockDbSetUiState.mockClear();
      useSidebarInboxStore.getState().prune(new Set(["ws-1"]));
      expect(mockDbSetUiState).not.toHaveBeenCalled();
    });

    it("load() round-trips snoozed entries and drops malformed ones", async () => {
      mockDbGetUiState.mockResolvedValue(
        JSON.stringify({
          settled: [{ id: "ws-1", at: 500, workEndedAt: 100 }],
          snoozed: [
            { id: "ws-2", at: 1, until: 2 },
            { id: "ws-3", at: 1 }, // no `until`
            { id: 42, at: 1, until: 2 },
            "nope",
            null,
          ],
          keepActive: [],
          activity: {},
        }),
      );
      await useSidebarInboxStore.getState().load();
      const s = useSidebarInboxStore.getState();
      expect(s.settled).toEqual([{ id: "ws-1", at: 500, workEndedAt: 100 }]);
      expect(s.snoozed).toEqual([{ id: "ws-2", at: 1, until: 2 }]);
    });

    it("load() back-compat: both older persisted shapes yield an empty snooze shelf", async () => {
      // Oldest shape: a bare array of settled entries.
      mockDbGetUiState.mockResolvedValue(
        JSON.stringify([{ id: "ws-1", at: 500 }]),
      );
      await useSidebarInboxStore.getState().load();
      let s = useSidebarInboxStore.getState();
      expect(s.settled).toEqual([{ id: "ws-1", at: 500 }]);
      expect(s.snoozed).toEqual([]);
      expect(s.keepActive).toEqual({});
      expect(s.activity).toEqual({});

      // Pre-snooze object shape: {settled, keepActive, activity}, no `snoozed`.
      __resetSidebarInboxStoreForTests();
      mockDbGetUiState.mockResolvedValue(
        JSON.stringify({
          settled: [{ id: "ws-1", at: 500 }],
          keepActive: ["ws-2"],
          activity: { "ws-3": 12345 },
        }),
      );
      await useSidebarInboxStore.getState().load();
      s = useSidebarInboxStore.getState();
      expect(s.settled).toEqual([{ id: "ws-1", at: 500 }]);
      expect(s.snoozed).toEqual([]);
      expect(s.keepActive).toEqual({ "ws-2": true });
      expect(s.activity).toEqual({ "ws-3": 12345 });
    });

    it("mutations of the other shelves carry the snooze list through untouched", () => {
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      const at = useSidebarInboxStore.getState().snoozed[0]!.at;

      useSidebarInboxStore.getState().settle("ws-2");
      expect(lastPersistedBlob().snoozed).toEqual([
        { id: "ws-1", at, until: 5_000 },
      ]);

      useSidebarInboxStore.getState().unsettle("ws-2", "user");
      expect(lastPersistedBlob().snoozed).toEqual([
        { id: "ws-1", at, until: 5_000 },
      ]);

      useSidebarInboxStore.getState().noteActivity("ws-2", 1_000_000);
      expect(lastPersistedBlob().snoozed).toEqual([
        { id: "ws-1", at, until: 5_000 },
      ]);
      expect(useSidebarInboxStore.getState().snoozed.map((e) => e.id)).toEqual([
        "ws-1",
      ]);
    });

    it("__resetSidebarInboxStoreForTests() clears the snooze shelf too", () => {
      useSidebarInboxStore.getState().snooze("ws-1", 5_000);
      expect(useSidebarInboxStore.getState().snoozed).toHaveLength(1);
      __resetSidebarInboxStoreForTests();
      expect(useSidebarInboxStore.getState().snoozed).toEqual([]);
    });
  });
});
