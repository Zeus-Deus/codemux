/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, act, cleanup } from "@testing-library/react";
import { StrictMode } from "react";

import type { WorkspaceSyncView } from "@/tauri/commands";

// Each call resolves with the current IPC snapshot unless a test
// supplies a pending request or transient error.
let mockNextRows: WorkspaceSyncView[] = [];
const listSpy = vi.fn<() => Promise<WorkspaceSyncView[]>>(() =>
  Promise.resolve(mockNextRows),
);

vi.mock("@/tauri/commands", () => ({
  workspacesSyncList: () => listSpy(),
}));

import {
  __resetWorkspacesSyncStoreForTests,
  useWorkspacesSync,
  useWorkspacesSyncStatus,
  useWorkspacesSyncStore,
} from "./workspaces-sync-store";

function makeRow(
  partial: Partial<WorkspaceSyncView> & { id: number },
): WorkspaceSyncView {
  return {
    id: partial.id,
    server_id: partial.server_id ?? null,
    workspace_id: partial.workspace_id ?? null,
    title: partial.title ?? `row-${partial.id}`,
    host_server_id: partial.host_server_id ?? null,
    project_path: partial.project_path ?? null,
    project_remote: partial.project_remote ?? null,
    git_branch: partial.git_branch ?? null,
    git_head_sha: partial.git_head_sha ?? null,
    project_uid: partial.project_uid ?? null,
    workspace_kind: partial.workspace_kind ?? null,
    default_branch: partial.default_branch ?? null,
    origin_path: partial.origin_path ?? null,
    created_at: partial.created_at ?? "2026-01-01T00:00:00Z",
    updated_at: partial.updated_at ?? "2026-01-01T00:00:00Z",
    dirty: partial.dirty ?? false,
  };
}

describe("useWorkspacesSync subscriber wiring", () => {
  beforeEach(() => {
    __resetWorkspacesSyncStoreForTests();
    listSpy.mockReset();
    listSpy.mockImplementation(() => Promise.resolve(mockNextRows));
    mockNextRows = [];
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    __resetWorkspacesSyncStoreForTests();
    vi.useRealTimers();
  });

  it.each([
    { name: "empty", rows: [] },
    { name: "populated", rows: [makeRow({ id: 1 }), makeRow({ id: 2 })] },
  ])("reuses equal $name rows from fresh IPC snapshots", async ({ rows }) => {
    mockNextRows = rows;
    const renderRows = vi.fn(() => useWorkspacesSync());
    const { result } = renderHook(renderRows);
    await act(async () => {
      await Promise.resolve();
    });
    const previousRows = result.current;
    const renderCount = renderRows.mock.calls.length;

    mockNextRows = rows.map((row) => ({ ...row }));
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });

    expect(listSpy).toHaveBeenCalledTimes(2);
    expect(result.current).toBe(previousRows);
    expect(renderRows).toHaveBeenCalledTimes(renderCount);
  });

  it.each(Object.keys(makeRow({ id: 1 })) as (keyof WorkspaceSyncView)[])(
    "publishes changes to %s without mutating previous rows",
    async (key) => {
      const row = makeRow({ id: 1 });
      mockNextRows = [row];
      await useWorkspacesSyncStore.getState().refresh();
      const previousRows = useWorkspacesSyncStore.getState().rows;
      const value = row[key];
      const changedValue = typeof value === "number" ? value + 1
        : typeof value === "boolean" ? !value
        : `${value ?? ""}-changed`;
      mockNextRows = [{ ...row, [key]: changedValue }];

      await useWorkspacesSyncStore.getState().refresh();

      expect(useWorkspacesSyncStore.getState().rows).toBe(mockNextRows);
      expect(useWorkspacesSyncStore.getState().rows).not.toBe(previousRows);
      expect(previousRows[0]?.[key]).toBe(value);
    },
  );

  it("publishes row additions, removals, and ordering changes", async () => {
    const a = makeRow({ id: 1 });
    const b = makeRow({ id: 2 });
    mockNextRows = [a, b];
    await useWorkspacesSyncStore.getState().refresh();

    for (const rows of [[b, a], [b], [b, a], []]) {
      mockNextRows = rows.map((row) => ({ ...row }));
      await useWorkspacesSyncStore.getState().refresh();
      expect(useWorkspacesSyncStore.getState().rows).toBe(mockNextRows);
    }
  });

  it("compares nested fields by reference without hiding future IPC fields", async () => {
    // Extra metadata models a future IPC field without weakening the row type.
    const row = { ...makeRow({ id: 1 }), metadata: { device: "laptop" } };
    mockNextRows = [row];
    await useWorkspacesSyncStore.getState().refresh();
    const previousRows = useWorkspacesSyncStore.getState().rows;

    mockNextRows = [{ ...row }];
    await useWorkspacesSyncStore.getState().refresh();
    expect(useWorkspacesSyncStore.getState().rows).toBe(previousRows);

    const freshRow = { ...row, metadata: { ...row.metadata } };
    mockNextRows = [freshRow];
    await useWorkspacesSyncStore.getState().refresh();
    expect(useWorkspacesSyncStore.getState().rows).toBe(mockNextRows);
    expect(useWorkspacesSyncStore.getState().rows[0]).toBe(freshRow);
  });

  it("deduplicates refresh and imperative init without starting polling", async () => {
    let resolveRows!: (rows: WorkspaceSyncView[]) => void;
    listSpy.mockImplementationOnce(() => new Promise((resolve) => {
      resolveRows = resolve;
    }));

    const pending = useWorkspacesSyncStore.getState().init();
    expect(useWorkspacesSyncStore.getState().refresh()).toBe(pending);
    expect(useWorkspacesSyncStore.getState().init()).toBe(pending);
    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(useWorkspacesSyncStore.getState()).toMatchObject({
      loading: true, loaded: false, error: null,
    });

    mockNextRows = [makeRow({ id: 1 })];
    resolveRows(mockNextRows);
    await pending;
    await useWorkspacesSyncStore.getState().init();

    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
    expect(useWorkspacesSyncStore.getState()).toMatchObject({
      rows: mockNextRows, loading: false, loaded: true, error: null,
    });
  });

  it.each(["database unavailable", new Error("database unavailable")])(
    "retains cached rows on error and surfaces recovery loading (%s)",
    async (error) => {
      mockNextRows = [makeRow({ id: 1 })];
      await useWorkspacesSyncStore.getState().refresh();
      const previousRows = useWorkspacesSyncStore.getState().rows;
      const { result } = renderHook(() => useWorkspacesSyncStatus());
      listSpy.mockRejectedValueOnce(error);

      await act(async () => {
        await expect(result.current.refresh()).resolves.toBeUndefined();
      });
      expect(result.current.rows).toBe(previousRows);
      expect(result.current).toMatchObject({
        loading: false, loaded: true, error: String(error),
      });

      mockNextRows = mockNextRows.map((row) => ({ ...row }));
      let pending!: Promise<void>;
      act(() => {
        pending = result.current.refresh();
      });
      expect(result.current).toMatchObject({
        loading: true, loaded: true, error: null,
      });
      await act(async () => {
        await pending;
      });
      expect(result.current.rows).toBe(previousRows);
      expect(result.current).toMatchObject({
        loading: false, loaded: true, error: null,
      });
    },
  );

  it("marks an initial failure as loaded and keeps init passive afterwards", async () => {
    listSpy.mockRejectedValueOnce("database unavailable");
    await expect(useWorkspacesSyncStore.getState().init()).resolves.toBeUndefined();
    await useWorkspacesSyncStore.getState().init();

    expect(useWorkspacesSyncStore.getState()).toMatchObject({
      rows: [], loading: false, loaded: true, error: "database unavailable",
    });
    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("fetches on mount even after the store has already loaded once", async () => {
    // Simulate the bug's exact scenario:
    //   1. Some earlier interaction (e.g. an imperative `init()` call,
    //      or a previous mount-unmount cycle) populated the store with
    //      an empty list.
    //   2. The background `hosts_inventory` poller (Rust side) THEN
    //      inserts sibling rows into the SQLite table.
    //   3. The user opens the overview — the hook must re-fetch so
    //      those new rows render. Pre-fix, the hook short-circuited
    //      on `loaded === true` and the bucket showed "0 workspaces".
    mockNextRows = [];
    await act(async () => {
      await useWorkspacesSyncStore.getState().refresh();
    });
    expect(useWorkspacesSyncStore.getState().rows).toEqual([]);
    expect(useWorkspacesSyncStore.getState().loaded).toBe(true);

    // Now the Rust background poller inserts a sibling row.
    mockNextRows = [
      makeRow({ id: 31, host_server_id: "2", title: "passpage-ui-polish" }),
    ];

    listSpy.mockClear();
    const { result } = renderHook(() => useWorkspacesSync());

    // Let the mount-effect's refresh resolve.
    await act(async () => {
      await Promise.resolve();
    });

    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(result.current).toHaveLength(1);
    expect(result.current[0]?.host_server_id).toBe("2");
  });

  it("polls every 5 s while at least one subscriber is mounted", async () => {
    mockNextRows = [];
    const { unmount } = renderHook(() => useWorkspacesSync());
    await act(async () => {
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);

    // Advance past two polling intervals — each should fire one call.
    mockNextRows = [makeRow({ id: 1 })];
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);

    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(3);

    // Unmount: remove the interval when no subscriber is watching.
    unmount();
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(3);
  });

  it("refreshes immediately and restarts polling when reopened", async () => {
    const first = renderHook(() => useWorkspacesSync());
    await act(async () => {
      await Promise.resolve();
    });
    first.unmount();
    expect(vi.getTimerCount()).toBe(0);

    await act(async () => {
      vi.advanceTimersByTime(10_000);
    });
    expect(listSpy).toHaveBeenCalledTimes(1);

    mockNextRows = [makeRow({ id: 2 })];
    const reopened = renderHook(() => useWorkspacesSync());
    await act(async () => {
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);
    expect(reopened.result.current).toEqual(mockNextRows);
    expect(vi.getTimerCount()).toBe(1);

    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(3);
    reopened.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("rearms one polling interval after StrictMode effect replay", async () => {
    const { result, unmount } = renderHook(() => useWorkspacesSync(), {
      wrapper: StrictMode,
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(1);

    mockNextRows = [makeRow({ id: 3 })];
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);
    expect(result.current).toEqual(mockNextRows);

    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("ref-counts across multiple concurrent subscribers", async () => {
    mockNextRows = [];
    const a = renderHook(() => useWorkspacesSync());
    const b = renderHook(() => useWorkspacesSync());
    // Both mounts kicked a refresh; in-flight dedupe collapses them
    // to a single fetch.
    await act(async () => {
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(1);

    // Unmount one — the other still drives polling.
    a.unmount();
    expect(vi.getTimerCount()).toBe(1);
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);

    // Unmount the last subscriber — polling stops.
    b.unmount();
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);
  });
});
