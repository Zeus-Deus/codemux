import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

import type { FollowUpTaskRow } from "@/tauri/commands";

import {
  resetFollowUpStoreForTests,
  useFollowUpRows,
  useFollowUpStore,
} from "./follow-up-store";

const commands = vi.hoisted(() => ({
  followUpList: vi.fn(),
  followUpProviders: vi.fn(),
}));
vi.mock("@/tauri/commands", () => commands);

const events = vi.hoisted(() => ({
  listener: null as ((payload: { parent_thread_id: string }) => void) | null,
}));
vi.mock("@/tauri/events", () => ({
  onFollowUpChanged: vi.fn(
    (cb: (payload: { parent_thread_id: string }) => void) => {
      events.listener = cb;
      return Promise.resolve(() => {});
    },
  ),
}));

function row(outcome: string): FollowUpTaskRow {
  return {
    child_thread_id: "child",
    parent_thread_id: "parent",
    tool_call_id: "call",
    index: 0,
    workspace_id: "ws-child",
    parent_workspace_id: "ws-parent",
    repo_path: "/repo",
    worktree_path: "/wt",
    branch: "branch",
    base_sha: "a".repeat(40),
    provider: "codex",
    title: "Task",
    outcome,
    additions: null,
    deletions: null,
    commits: null,
    uncommitted: null,
    merged: false,
    created_at_ms: 1,
  } as FollowUpTaskRow;
}

/** A `followUpList` call the test resolves by hand. */
function deferredList() {
  const pending: Array<(rows: FollowUpTaskRow[]) => void> = [];
  commands.followUpList.mockImplementation(
    () => new Promise<FollowUpTaskRow[]>((resolve) => pending.push(resolve)),
  );
  return pending;
}

const outcomes = () =>
  (useFollowUpStore.getState().rows.parent ?? []).map((r) => r.outcome);

beforeEach(() => {
  resetFollowUpStoreForTests();
  events.listener = null;
  commands.followUpList.mockReset();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("follow-up store refresh", () => {
  it("runs one trailing read when a change arrives during a read, and its rows win", async () => {
    const pending = deferredList();
    const first = useFollowUpStore.getState().refresh("parent");
    expect(commands.followUpList).toHaveBeenCalledTimes(1);

    // Two changes while the first read is in flight coalesce into one more read.
    const second = useFollowUpStore.getState().refresh("parent");
    void useFollowUpStore.getState().refresh("parent");
    expect(second).toBe(first);
    expect(commands.followUpList).toHaveBeenCalledTimes(1);

    // The first read returns rows from before the change.
    await act(async () => pending[0]([row("running")]));
    await waitFor(() => expect(commands.followUpList).toHaveBeenCalledTimes(2));
    expect(outcomes()).toEqual(["running"]);

    await act(async () => pending[1]([row("completed")]));
    await first;
    expect(outcomes()).toEqual(["completed"]);
    expect(commands.followUpList).toHaveBeenCalledTimes(2);

    // Settled: the next refresh is a fresh read.
    const third = useFollowUpStore.getState().refresh("parent");
    expect(third).not.toBe(first);
    expect(commands.followUpList).toHaveBeenCalledTimes(3);
    await act(async () => pending[2]([row("completed")]));
    await third;
  });

  it("does not ignore a change event that arrives during the first read", async () => {
    const pending = deferredList();
    renderHook(() => useFollowUpRows("parent"));
    await waitFor(() => expect(events.listener).not.toBeNull());
    expect(commands.followUpList).toHaveBeenCalledTimes(1);
    expect(useFollowUpStore.getState().rows).not.toHaveProperty("parent");

    // No rows yet, but the thread is watched: the event queues a read.
    act(() => events.listener!({ parent_thread_id: "parent" }));
    // Threads nobody looked at are not read.
    act(() => events.listener!({ parent_thread_id: "elsewhere" }));

    await act(async () => pending[0]([]));
    await waitFor(() => expect(commands.followUpList).toHaveBeenCalledTimes(2));
    await act(async () => pending[1]([row("running")]));
    await waitFor(() => expect(outcomes()).toEqual(["running"]));
    expect(commands.followUpList).not.toHaveBeenCalledWith("elsewhere");
  });

  it("keeps reading after a failed read when a change is queued", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const rejects: Array<(error: unknown) => void> = [];
    const resolves: Array<(rows: FollowUpTaskRow[]) => void> = [];
    commands.followUpList
      .mockImplementationOnce(
        () => new Promise((_, reject) => rejects.push(reject)),
      )
      .mockImplementationOnce(
        () => new Promise((resolve) => resolves.push(resolve)),
      );
    const first = useFollowUpStore.getState().refresh("parent");
    void useFollowUpStore.getState().refresh("parent");
    await act(async () => rejects[0](new Error("boom")));
    await waitFor(() => expect(commands.followUpList).toHaveBeenCalledTimes(2));
    await act(async () => resolves[0]([row("failed")]));
    await first;
    expect(outcomes()).toEqual(["failed"]);
    warn.mockRestore();
  });
});
