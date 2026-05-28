/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, act, cleanup } from "@testing-library/react";

import type { WorkspaceSyncView } from "@/tauri/commands";

// ── Mock the Tauri command so each call resolves with whatever the
//    current test put in `mockNextRows`. We track the call count to
//    pin down the bug being fixed: previously the polling interval
//    fired but the body short-circuited because `subscriberCount`
//    was stuck at 0 (no useEffect ever called the increment helper),
//    so the store never re-read the DB after the first load.
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
    created_at: partial.created_at ?? "2026-01-01T00:00:00Z",
    updated_at: partial.updated_at ?? "2026-01-01T00:00:00Z",
    dirty: partial.dirty ?? false,
  };
}

describe("useWorkspacesSync subscriber wiring", () => {
  beforeEach(() => {
    __resetWorkspacesSyncStoreForTests();
    listSpy.mockClear();
    mockNextRows = [];
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
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

    // Unmount: polling skips because no subscriber is watching.
    unmount();
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(3);
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
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);

    // Unmount the last subscriber — polling skips.
    b.unmount();
    await act(async () => {
      vi.advanceTimersByTime(10_000);
      await Promise.resolve();
    });
    expect(listSpy).toHaveBeenCalledTimes(2);
  });
});
