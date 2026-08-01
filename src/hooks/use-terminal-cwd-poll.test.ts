import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

vi.mock("@/tauri/commands", () => ({
  terminalSessionCwds: vi.fn(async () => ({})),
}));

import { useAppStore } from "@/stores/app-store";
import { useTerminalCwdStore } from "@/stores/terminal-cwd-store";
import {
  collectLiveSessionIds,
  sameSessionSet,
  useTerminalCwdPoll,
} from "./use-terminal-cwd-poll";

// ── Fixtures ──

function makeWs(id: string, sessionIds: string[]): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: id,
    workspace_type: "standard",
    cwd: `/repos/${id}`,
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: `surface-${id}`,
    surfaces: [
      {
        surface_id: `surface-${id}`,
        title: id,
        active_pane_id: `pane-${sessionIds[0] ?? "none"}`,
        root: {
          kind: "split",
          pane_id: `split-${id}`,
          direction: "horizontal",
          child_sizes: sessionIds.map(() => 1),
          children: sessionIds.map((sessionId) => ({
            kind: "terminal" as const,
            pane_id: `pane-${sessionId}`,
            session_id: sessionId,
            title: sessionId,
          })),
        },
      },
    ],
  };
}

function makeAppState(workspaces: WorkspaceSnapshot[]): AppStateSnapshot {
  return {
    schema_version: 1,
    active_workspace_id: workspaces[0]?.workspace_id ?? "",
    workspaces,
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    persistence: {} as AppStateSnapshot["persistence"],
    config: {} as AppStateSnapshot["config"],
  };
}

// ── Pure helpers ──

describe("collectLiveSessionIds", () => {
  it("walks every workspace, not just the active one", () => {
    const state = makeAppState([
      makeWs("ws-a", ["s1", "s2"]),
      makeWs("ws-b", ["s3"]),
    ]);
    expect([...collectLiveSessionIds(state)].sort()).toEqual(["s1", "s2", "s3"]);
  });

  it("returns an empty set for a snapshot with no terminals", () => {
    expect(collectLiveSessionIds(makeAppState([]))).toEqual(new Set());
  });
});

describe("sameSessionSet", () => {
  it("compares by membership, not identity", () => {
    expect(sameSessionSet(new Set(["a", "b"]), new Set(["b", "a"]))).toBe(true);
    expect(sameSessionSet(new Set(["a"]), new Set(["a", "b"]))).toBe(false);
    expect(sameSessionSet(new Set(["a"]), new Set(["b"]))).toBe(false);
  });
});

// ── Prune scheduling ──
//
// The pane-tree walk used to run on every 2 Hz tick regardless of whether
// anything could possibly have been freed. It must now run only when the
// snapshot identity changes, and write to the store only when the live
// session set genuinely moved.

describe("useTerminalCwdPoll pruning", () => {
  let pruneSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.useFakeTimers();
    useAppStore.setState({ appState: null });
    useTerminalCwdStore.setState({ cwds: {} });
    pruneSpy = vi.fn();
    // The hook reads the store through `getState()`, so replacing the
    // action on the state object is enough to observe the calls.
    useTerminalCwdStore.setState({ pruneCwds: pruneSpy as never });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("prunes once and then stays quiet while the snapshot is unchanged", async () => {
    useAppStore.setState({ appState: makeAppState([makeWs("ws-a", ["s1"])]) });
    renderHook(() => useTerminalCwdPoll());

    expect(pruneSpy).toHaveBeenCalledTimes(1);
    expect(pruneSpy).toHaveBeenCalledWith(new Set(["s1"]));

    // Ten ticks over the same snapshot object — the shape `shareStructural`
    // guarantees for a no-op backend emit. Async advancement so the in-flight
    // IPC settles and each tick really runs.
    for (let i = 0; i < 10; i += 1) await vi.advanceTimersByTimeAsync(2000);
    expect(pruneSpy).toHaveBeenCalledTimes(1);
  });

  it("does not prune when a new snapshot carries the same sessions", async () => {
    useAppStore.setState({ appState: makeAppState([makeWs("ws-a", ["s1"])]) });
    renderHook(() => useTerminalCwdPoll());
    expect(pruneSpy).toHaveBeenCalledTimes(1);

    // A different object (e.g. a git-metadata emit) with an identical
    // session set: the walk runs, the store write does not.
    useAppStore.setState({ appState: makeAppState([makeWs("ws-a", ["s1"])]) });
    await vi.advanceTimersByTimeAsync(2000);
    expect(pruneSpy).toHaveBeenCalledTimes(1);
  });

  it("prunes again when a session disappears", async () => {
    useAppStore.setState({
      appState: makeAppState([makeWs("ws-a", ["s1", "s2"])]),
    });
    renderHook(() => useTerminalCwdPoll());
    expect(pruneSpy).toHaveBeenCalledTimes(1);

    useAppStore.setState({ appState: makeAppState([makeWs("ws-a", ["s1"])]) });
    await vi.advanceTimersByTimeAsync(2000);
    expect(pruneSpy).toHaveBeenCalledTimes(2);
    expect(pruneSpy).toHaveBeenLastCalledWith(new Set(["s1"]));
  });
});
