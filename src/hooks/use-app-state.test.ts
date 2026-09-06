import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act, cleanup } from "@testing-library/react";

const getAppStateMock = vi.hoisted(() =>
  vi.fn<() => Promise<AppStateSnapshot>>(() => new Promise<never>(() => {})),
);

vi.mock("@/tauri/commands", () => ({
  getAppState: getAppStateMock,
}));

let deliverSnapshot: ((payload: AppStateSnapshot) => void) | null = null;
let deliverDelta: ((payload: RevisionedDelta) => void) | null = null;
let deliverRevision: ((payload: RevisionHeartbeat) => void) | null = null;

vi.mock("@/tauri/events", () => ({
  onAppStateChanged: (cb: (payload: AppStateSnapshot) => void) => {
    deliverSnapshot = cb;
    return Promise.resolve(() => {});
  },
  onAppStateDelta: (cb: (payload: RevisionedDelta) => void) => {
    deliverDelta = cb;
    return Promise.resolve(() => {});
  },
  onAppStateRevision: (cb: (payload: RevisionHeartbeat) => void) => {
    deliverRevision = cb;
    return Promise.resolve(() => {});
  },
}));

import { confirmsPendingActivation, useAppStateInit } from "./use-app-state";
import { useAppStore, DELTA_REORDER_WINDOW_MS } from "@/stores/app-store";
import {
  abandonInteraction,
  beginInteraction,
  clearTraces,
  configureInteractionTrace,
  getTraces,
  mark,
  markOpenInteraction,
} from "@/lib/perf/interaction-trace";
import type {
  AppStateDelta,
  AppStateSnapshot,
  RevisionedDelta,
  RevisionHeartbeat,
  WorkspaceSnapshot,
} from "@/tauri/types";

function makeWs(workspaceId: string): WorkspaceSnapshot {
  return {
    workspace_id: workspaceId,
    title: workspaceId,
    workspace_type: "standard",
    cwd: "/tmp/project",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
  };
}

function makeAppState(activeWorkspaceId: string): AppStateSnapshot {
  return {
    schema_version: 1,
    active_workspace_id: activeWorkspaceId,
    workspaces: [makeWs("ws-A"), makeWs("ws-B")],
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    persistence: {
      schema_version: 1,
      stores_layout_metadata: true,
      stores_terminal_metadata: true,
      stores_live_process_state: true,
    },
    config: {
      config_version: 1,
      default_shell: null,
      theme_source: "default",
      linux_first: false,
      notification_sound_enabled: true,
      ai_commit_message_enabled: false,
      ai_commit_message_cli: null,
      ai_commit_message_model: null,
      ai_resolver_enabled: false,
      ai_resolver_cli: null,
      ai_resolver_model: null,
      ai_resolver_strategy: "auto",
    },
  };
}

describe("confirmsPendingActivation", () => {
  it("is true only for the snapshot naming the pending workspace", () => {
    expect(confirmsPendingActivation({ active_workspace_id: "ws-B" }, "ws-B")).toBe(true);
    expect(confirmsPendingActivation({ active_workspace_id: "ws-A" }, "ws-B")).toBe(false);
  });

  it("is false when nothing is pending", () => {
    expect(confirmsPendingActivation({ active_workspace_id: "ws-B" }, null)).toBe(false);
  });
});

describe("useAppStateInit — activation snapshots bypass the debounce", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    deliverSnapshot = null;
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
  });

  afterEach(() => {
    // Unmount: a still-mounted hook keeps subscribing to the store, so a
    // later test's resync would be serviced by several listeners at once.
    cleanup();
    vi.useRealTimers();
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
  });

  async function mountListener() {
    renderHook(() => useAppStateInit());
    // `useTauriEvent` subscribes in an effect and resolves a promise.
    await act(async () => {
      await Promise.resolve();
    });
    expect(deliverSnapshot).not.toBeNull();
  }

  it("commits the confirming snapshot synchronously", async () => {
    await mountListener();
    useAppStore.getState().beginPendingActivation("ws-B");

    act(() => {
      deliverSnapshot!(makeAppState("ws-B"));
    });

    // No timer has run: the commit happened in the delivery task itself.
    expect(useAppStore.getState().appState?.active_workspace_id).toBe("ws-B");
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
  });

  it("still debounces snapshots that do not confirm a selection", async () => {
    await mountListener();

    act(() => {
      deliverSnapshot!(makeAppState("ws-A"));
    });
    expect(useAppStore.getState().appState).toBeNull();

    act(() => {
      vi.advanceTimersByTime(16);
    });
    expect(useAppStore.getState().appState?.active_workspace_id).toBe("ws-A");
  });

  it("is not starved by a stream of background emits", async () => {
    // A trailing-edge debounce reschedules on every event, so under sustained
    // churn the activation snapshot would otherwise never commit.
    await mountListener();
    useAppStore.getState().beginPendingActivation("ws-B");

    act(() => {
      for (let i = 0; i < 5; i += 1) {
        deliverSnapshot!(makeAppState("ws-A"));
        vi.advanceTimersByTime(8);
      }
      deliverSnapshot!(makeAppState("ws-B"));
    });

    expect(useAppStore.getState().appState?.active_workspace_id).toBe("ws-B");

    // The superseded debounce timer must not fire a stale commit afterwards.
    act(() => {
      vi.advanceTimersByTime(64);
    });
    expect(useAppStore.getState().appState?.active_workspace_id).toBe("ws-B");
  });
});

// ── Domain deltas + revision heartbeat (Phase 6) ────────────────────────

describe("useAppStateInit — deltas, gaps and the revision heartbeat", () => {
  const gitDelta: AppStateDelta = {
    domain: "workspace_git",
    workspace_id: "ws-A",
    git: {
      is_git: true,
      git_branch: "feature",
      git_ahead: 1,
      git_behind: 0,
      git_additions: 0,
      git_deletions: 0,
      git_changed_files: 0,
    },
  };

  function resetStore(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
      backendInstance: null,
      resyncInFlight: false,
      resyncRequestId: 0,
      deltaBuffer: new Map(),
      gapWindowId: 0,
    });
  }

  beforeEach(() => {
    vi.useFakeTimers();
    deliverSnapshot = null;
    deliverDelta = null;
    deliverRevision = null;
    getAppStateMock.mockReset();
    getAppStateMock.mockImplementation(() => new Promise<never>(() => {}));
    resetStore();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    resetStore();
  });

  async function mountListeners() {
    renderHook(() => useAppStateInit());
    await act(async () => {
      await Promise.resolve();
    });
    expect(deliverDelta).not.toBeNull();
    expect(deliverRevision).not.toBeNull();
  }

  function snapshotAt(revision: number, activeWorkspaceId = "ws-A"): AppStateSnapshot {
    const snapshot = makeAppState(activeWorkspaceId);
    snapshot.snapshot_revision = revision;
    return snapshot;
  }

  it("applies a delta immediately, without the 16 ms window", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });

    act(() => {
      deliverDelta!({ revision: 11, delta: gitDelta });
    });

    // No timer advanced between delivery and assertion.
    expect(useAppStore.getState().appState!.workspaces[0].git_branch).toBe("feature");
    expect(useAppStore.getState().lastSeenRevision).toBe(11);
  });

  it("flushes a debounced snapshot before applying a delta, so ordering holds", async () => {
    // The snapshot was delivered first and carries the lower revision.
    // Committing it after the delta would leave the baseline behind the
    // stream and make the delta look like a gap.
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      // Still inside the coalescing window — nothing committed yet.
      deliverDelta!({ revision: 11, delta: gitDelta });
    });

    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(11);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
    expect(state.resyncRequestId).toBe(0);
  });

  it("fetches one full snapshot when the reorder window expires on a real gap", async () => {
    let resolveFetch: (snapshot: AppStateSnapshot) => void = () => {};
    getAppStateMock.mockImplementation(
      () => new Promise<AppStateSnapshot>((resolve) => (resolveFetch = resolve)),
    );

    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    // A forward gap buys a short window before it costs a full snapshot.
    act(() => {
      deliverDelta!({ revision: 14, delta: gitDelta });
    });
    expect(getAppStateMock).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(DELTA_REORDER_WINDOW_MS);
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);

    // Deltas arriving while the fetch is out are buffered, not dropped: they
    // may be stamped above the revision the snapshot reports.
    act(() => {
      deliverDelta!({ revision: 21, delta: gitDelta });
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().lastSeenRevision).toBe(10);

    await act(async () => {
      resolveFetch(snapshotAt(20));
      await Promise.resolve();
      await Promise.resolve();
    });
    // 14 was below the baseline and dropped with it; 21 replayed on top.
    expect(useAppStore.getState().lastSeenRevision).toBe(21);
    expect(useAppStore.getState().resyncInFlight).toBe(false);
  });

  it("absorbs an out-of-order delta without fetching when the gap fills", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    // The delta overtook the snapshot it belongs behind.
    act(() => {
      deliverDelta!({ revision: 12, delta: gitDelta });
      deliverSnapshot!(snapshotAt(11));
      vi.advanceTimersByTime(16);
    });

    expect(useAppStore.getState().lastSeenRevision).toBe(12);
    expect(useAppStore.getState().appState!.workspaces[0].git_branch).toBe("feature");

    // The window closed on its own, so no timer can still open a resync.
    act(() => {
      vi.advanceTimersByTime(DELTA_REORDER_WINDOW_MS * 2);
    });
    expect(getAppStateMock).not.toHaveBeenCalled();
    expect(useAppStore.getState().resyncInFlight).toBe(false);
  });

  it("resyncs when the heartbeat reports a revision we never saw", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    // Level: the backend is where we already are — nothing to do.
    act(() => {
      deliverRevision!({ revision: 10 });
    });
    expect(getAppStateMock).not.toHaveBeenCalled();

    act(() => {
      deliverRevision!({ revision: 12 });
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);

    // Single-flight: a second heartbeat while the fetch is out adds nothing.
    act(() => {
      deliverRevision!({ revision: 13 });
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);
  });

  it("does not resync from a heartbeat before the first snapshot", async () => {
    await mountListeners();
    getAppStateMock.mockClear();
    act(() => {
      deliverRevision!({ revision: 5 });
    });
    expect(getAppStateMock).not.toHaveBeenCalled();
  });

  // ── Backend restart (the live-restart web-remote freeze) ──
  //
  // The revision counter restarts at 0 with the process. A web-remote page
  // outlives that restart, so `payload.revision > lastSeenRevision` — the
  // whole heartbeat condition — is FALSE for every heartbeat the new backend
  // sends, and the one mechanism that exists to notice "we are behind" never
  // fires. The page froze with a heartbeat arriving every minute.

  it("resyncs on a heartbeat from a restarted backend even though its revision is LOWER", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!({ ...snapshotAt(500), snapshot_instance: "instance-1" });
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    // The restarted backend is at revision 4 — far below the 500 we hold.
    act(() => {
      deliverRevision!({ revision: 4, instance: "instance-2" });
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);
  });

  it("a same-instance heartbeat at a lower revision still resyncs nothing", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!({ ...snapshotAt(500), snapshot_instance: "instance-1" });
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    act(() => {
      deliverRevision!({ revision: 499, instance: "instance-1" });
    });
    expect(getAppStateMock).not.toHaveBeenCalled();
  });

  it("an unstamped heartbeat keeps the plain revision comparison", async () => {
    // Older backend / mock: no token, so the only signal is the number.
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    getAppStateMock.mockClear();

    act(() => {
      deliverRevision!({ revision: 9 });
    });
    expect(getAppStateMock).not.toHaveBeenCalled();

    act(() => {
      deliverRevision!({ revision: 11 });
    });
    expect(getAppStateMock).toHaveBeenCalledTimes(1);
  });

  it("the reseeded snapshot after a restart applies and rebases the counter", async () => {
    // The reconnect path: `reseedOnReconnect` refetches and pushes the result
    // through this same `app-state-changed` handler. Before the instance
    // token, that snapshot's low revision read as stale and was dropped —
    // the page kept rendering the pre-restart world.
    await mountListeners();
    act(() => {
      deliverSnapshot!({ ...snapshotAt(500, "ws-A"), snapshot_instance: "instance-1" });
      vi.advanceTimersByTime(16);
    });
    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-A");

    act(() => {
      deliverSnapshot!({ ...snapshotAt(2, "ws-B"), snapshot_instance: "instance-2" });
      vi.advanceTimersByTime(16);
    });

    const state = useAppStore.getState();
    expect(state.appState!.active_workspace_id).toBe("ws-B");
    expect(state.lastSeenRevision).toBe(2);
  });

  it("deltas from the restarted backend flow again once the reseed lands", async () => {
    // End to end: restart → reseed → the next contiguous delta applies. This
    // is the property the freeze actually denied — not just one snapshot, but
    // every update afterwards.
    await mountListeners();
    act(() => {
      deliverSnapshot!({ ...snapshotAt(500), snapshot_instance: "instance-1" });
      vi.advanceTimersByTime(16);
    });

    act(() => {
      deliverSnapshot!({ ...snapshotAt(2), snapshot_instance: "instance-2" });
      vi.advanceTimersByTime(16);
    });
    act(() => {
      deliverDelta!({ revision: 3, instance: "instance-2", delta: gitDelta });
    });

    const state = useAppStore.getState();
    expect(state.lastSeenRevision).toBe(3);
    expect(state.appState!.workspaces[0].git_branch).toBe("feature");
  });
});

describe("useAppStateInit — activation arrives as a delta", () => {
  const activation: AppStateDelta = {
    domain: "active_workspace",
    workspace_id: "ws-B",
    previous_workspace_id: "ws-A",
    last_visited_at: 1_000,
    previous_last_visited_at: 1_000,
    cleared_review_pane_ids: [],
  };

  function resetStore(): void {
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
      backendInstance: null,
      resyncInFlight: false,
      resyncRequestId: 0,
      deltaBuffer: new Map(),
      gapWindowId: 0,
    });
  }

  beforeEach(() => {
    // `performance` and rAF are faked too so the trace's double-rAF paint
    // stamp and its span arithmetic run on the same clock as the timers.
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "clearTimeout",
        "setInterval",
        "clearInterval",
        "requestAnimationFrame",
        "cancelAnimationFrame",
        "performance",
        "Date",
      ],
    });
    deliverSnapshot = null;
    deliverDelta = null;
    deliverRevision = null;
    getAppStateMock.mockReset();
    getAppStateMock.mockImplementation(() => new Promise<never>(() => {}));
    resetStore();
    configureInteractionTrace({ enabled: true, console: false });
    clearTraces();
  });

  afterEach(() => {
    cleanup();
    configureInteractionTrace({ enabled: false, console: false });
    clearTraces();
    vi.useRealTimers();
    resetStore();
  });

  async function mountListeners() {
    renderHook(() => useAppStateInit());
    await act(async () => {
      await Promise.resolve();
    });
    expect(deliverDelta).not.toBeNull();
  }

  function snapshotAt(revision: number): AppStateSnapshot {
    const snapshot = makeAppState("ws-A");
    snapshot.snapshot_revision = revision;
    return snapshot;
  }

  it("confirms the optimistic selection without any full snapshot", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    act(() => {
      useAppStore.getState().beginPendingActivation("ws-B");
      deliverDelta!({ revision: 11, delta: activation });
    });

    // No timer advanced: the delta bypasses the coalescing window, exactly as
    // the confirming snapshot did.
    const state = useAppStore.getState();
    expect(state.appState!.active_workspace_id).toBe("ws-B");
    expect(state.pendingActiveWorkspaceId).toBeNull();
    expect(state.lastSeenRevision).toBe(11);
  });

  it("stamps the trace's state-event and commit phases so the switch trace closes", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });

    const id = beginInteraction("workspace-switch", { target: "ws-B" });
    act(() => {
      mark(id, "click");
      mark(id, "invoke-start");
      useAppStore.getState().beginPendingActivation("ws-B");
      // Optimistic paint lands before the backend answers.
      markOpenInteraction("pane-mounted", { target: "ws-B" });
      markOpenInteraction("pane-content-ready", { target: "ws-B", meta: { paneKind: 1 } });
      markOpenInteraction("pane-interactive", { target: "ws-B", meta: { paneKind: 1 } });
      vi.advanceTimersByTime(40); // double-rAF → painted
      mark(id, "invoke-returned");
      vi.advanceTimersByTime(20);
      deliverDelta!({ revision: 11, delta: activation });
    });

    const traces = getTraces();
    expect(traces).toHaveLength(1);
    expect(traces[0].complete).toBe(true);
    expect(traces[0].abandoned).toBe(false);
    // `state-committed` is the closing phase; both marks must be present for
    // the trace to close on the delta rather than the 3 s post-paint grace.
    const phases = traces[0].marks.map((m) => m.phase);
    expect(phases).toContain("snapshot-received");
    expect(phases).toContain("state-committed");
  });

  it("does not stamp state-committed for an activation delta that had to be buffered", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });

    const id = beginInteraction("workspace-switch", { target: "ws-B" });
    act(() => {
      mark(id, "click");
      mark(id, "invoke-start");
      useAppStore.getState().beginPendingActivation("ws-B");
      markOpenInteraction("pane-mounted", { target: "ws-B" });
      markOpenInteraction("pane-content-ready", { target: "ws-B", meta: { paneKind: 1 } });
      markOpenInteraction("pane-interactive", { target: "ws-B", meta: { paneKind: 1 } });
      vi.advanceTimersByTime(40); // double-rAF → painted
      mark(id, "invoke-returned");
      // Revision 12 arrives before 11 — held in the reorder buffer, not applied.
      deliverDelta!({ revision: 12, delta: activation });
    });
    expect(useAppStore.getState().lastSeenRevision).toBe(10);
    expect(getTraces()).toHaveLength(0);

    // The gap fills; draining commits the activation and the trace closes on
    // the message that actually committed it — the snapshot, stamped with the
    // DRAINED active id rather than its own stale one. This snapshot is not
    // itself a confirming one, so it rides the ordinary coalescing window.
    act(() => {
      deliverSnapshot!(snapshotAt(11));
      vi.advanceTimersByTime(16);
    });
    expect(useAppStore.getState().lastSeenRevision).toBe(12);
    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-B");
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    const traces = getTraces();
    expect(traces).toHaveLength(1);
    expect(traces[0].complete).toBe(true);
  });

  it("ignores non-activation deltas for trace purposes", async () => {
    await mountListeners();
    act(() => {
      deliverSnapshot!(snapshotAt(10));
      vi.advanceTimersByTime(16);
    });
    const id = beginInteraction("workspace-switch", { target: "ws-B" });
    act(() => {
      mark(id, "click");
      mark(id, "invoke-start");
      deliverDelta!({
        revision: 11,
        delta: { domain: "pane_status", pane_id: "p", status: "working" },
      });
    });
    // Still open: a pane-status delta is not the confirming state event.
    expect(getTraces()).toHaveLength(0);
    abandonInteraction(id);
  });
});
