import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn(),
}));

import { activateWorkspace } from "@/tauri/commands";
import { activateWorkspaceInteraction } from "./instrumented-activate";
import {
  clearTraces,
  configureInteractionTrace,
  getTraces,
  markOpenInteraction,
} from "./interaction-trace";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import {
  useChatDraftStore,
  type ChatDraft,
  type DraftId,
} from "@/stores/chat-draft-store";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

const mockActivateWorkspace = vi.mocked(activateWorkspace);

/** Double-rAF plus the post-paint grace, so a trace with no `state-committed`
 *  has definitely closed. */
const TRACE_SETTLE_MS = 40 + 3_000;

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

/** Seed a minimal draft body + active pointer, mirroring the state a user
 *  has while composing on the draft surface. */
function seedActiveDraft(draftId: string): void {
  useChatDraftStore.setState({
    draftsById: {
      [draftId]: { draftId } as unknown as ChatDraft,
    } as Record<DraftId, ChatDraft>,
    activeDraftId: draftId as unknown as DraftId,
  });
}

describe("activateWorkspaceInteraction", () => {
  beforeEach(() => {
    mockActivateWorkspace.mockReset();
    // The trace harness is on in dev builds; its timers and console output
    // are irrelevant here and would fight the fake clock.
    configureInteractionTrace({ enabled: false });
    useAppStore.setState({
      appState: makeAppState("ws-A"),
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
    useChatDraftStore.setState({ draftsById: {}, activeDraftId: null });
  });

  afterEach(() => {
    vi.useRealTimers();
    configureInteractionTrace({});
    useAppStore.setState({
      appState: null,
      pendingActiveWorkspaceId: null,
      pendingActivationAt: null,
      lastSeenRevision: 0,
    });
    useChatDraftStore.setState({ draftsById: {}, activeDraftId: null });
  });

  it("paints the selection before the invoke settles", async () => {
    // The exit gate: the pending id must flip in the click's own task, with
    // the invoke promise still unresolved and no snapshot yet applied.
    let resolveInvoke!: () => void;
    mockActivateWorkspace.mockReturnValue(
      new Promise<void>((resolve) => {
        resolveInvoke = resolve;
      }),
    );

    const pending = activateWorkspaceInteraction("ws-B");

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");
    // The backend snapshot has not moved.
    expect(useAppStore.getState().appState!.active_workspace_id).toBe("ws-A");
    expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-B");

    resolveInvoke();
    await pending;

    // Still pending — only a confirming snapshot reconciles it.
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
  });

  it("clears the active chat draft", () => {
    mockActivateWorkspace.mockResolvedValue(undefined);
    useChatDraftStore.setState({
      activeDraftId: "draft-1" as unknown as DraftId,
    });

    void activateWorkspaceInteraction("ws-B").catch(() => {});

    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("rolls the selection back when the invoke rejects", async () => {
    mockActivateWorkspace.mockRejectedValue(new Error("no such workspace"));

    const promise = activateWorkspaceInteraction("ws-B");
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");

    await expect(promise).rejects.toThrow("no such workspace");

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");
  });

  it("does not roll back a newer selection when an older invoke rejects", async () => {
    let rejectFirst!: (error: Error) => void;
    mockActivateWorkspace.mockReturnValueOnce(
      new Promise<void>((_resolve, reject) => {
        rejectFirst = reject;
      }),
    );
    mockActivateWorkspace.mockReturnValueOnce(new Promise<void>(() => {}));

    const first = activateWorkspaceInteraction("ws-B");
    const second = activateWorkspaceInteraction("ws-A");
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-A");

    rejectFirst(new Error("stale"));
    await expect(first).rejects.toThrow("stale");
    void second;

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-A");
  });

  it("restores the dismissed draft view when the invoke rejects", async () => {
    mockActivateWorkspace.mockRejectedValue(new Error("no such workspace"));
    seedActiveDraft("draft-1");

    const promise = activateWorkspaceInteraction("ws-B");
    // The draft view is dismissed optimistically with the selection...
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();

    await expect(promise).rejects.toThrow("no such workspace");

    // ...and comes back with the rolled-back selection, so the user is not
    // left on the old workspace with their composer gone.
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");
    expect(useChatDraftStore.getState().activeDraftId).toBe("draft-1");
  });

  it("does not restore the draft when a newer activation superseded the rejection", async () => {
    let rejectFirst!: (error: Error) => void;
    mockActivateWorkspace.mockReturnValueOnce(
      new Promise<void>((_resolve, reject) => {
        rejectFirst = reject;
      }),
    );
    mockActivateWorkspace.mockReturnValueOnce(new Promise<void>(() => {}));
    seedActiveDraft("draft-1");

    const first = activateWorkspaceInteraction("ws-B");
    void activateWorkspaceInteraction("ws-A").catch(() => {});
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-A");

    rejectFirst(new Error("stale"));
    await expect(first).rejects.toThrow("stale");

    // The newer activation owns the pending selection and the (re-cleared)
    // draft pointer; the stale rejection must not resurrect the old view.
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-A");
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("does not roll back or restore the draft when a newer activation of the SAME id is in flight", async () => {
    // Two rapid activations of one workspace: the pending-id check alone can't
    // tell them apart, so without sequence scoping the first call's rejection
    // would disarm the second call's timeout, clear its pending selection and
    // resurrect the draft view over its still-in-flight switch.
    let rejectFirst!: (error: Error) => void;
    mockActivateWorkspace.mockReturnValueOnce(
      new Promise<void>((_resolve, reject) => {
        rejectFirst = reject;
      }),
    );
    mockActivateWorkspace.mockReturnValueOnce(new Promise<void>(() => {}));
    seedActiveDraft("draft-1");

    const first = activateWorkspaceInteraction("ws-B");
    void activateWorkspaceInteraction("ws-B").catch(() => {});
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");

    rejectFirst(new Error("stale"));
    await expect(first).rejects.toThrow("stale");

    // The newer same-id activation still owns the pending selection and the
    // dismissed draft pointer.
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-B");
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("opens no trace when the target is already the rendered workspace", async () => {
    // Re-selecting the active workspace mounts no new pane tree, so nothing
    // would ever stamp `pane-mounted` — the trace would sit open for its whole
    // abandon timeout, absorbing long tasks that belong to the next thing the
    // user does.
    vi.useFakeTimers({
      toFake: [
        "setTimeout",
        "clearTimeout",
        "requestAnimationFrame",
        "cancelAnimationFrame",
        "performance",
        "Date",
      ],
    });
    configureInteractionTrace({ enabled: true, console: false });
    clearTraces();
    mockActivateWorkspace.mockResolvedValue(undefined);

    await activateWorkspaceInteraction("ws-A");
    markOpenInteraction("pane-mounted", { target: "ws-A" });
    vi.advanceTimersByTime(TRACE_SETTLE_MS);

    expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-A");
    expect(getTraces()).toHaveLength(0);

    // A switch to a different workspace still traces.
    await activateWorkspaceInteraction("ws-B");
    markOpenInteraction("pane-mounted", { target: "ws-B" });
    vi.advanceTimersByTime(TRACE_SETTLE_MS);
    expect(getTraces()).toHaveLength(1);
  });

  it("ages out a pending selection the backend never confirms", async () => {
    vi.useFakeTimers();
    mockActivateWorkspace.mockResolvedValue(undefined);

    const promise = activateWorkspaceInteraction("ws-B");
    await promise;
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-B");

    vi.advanceTimersByTime(5_000);

    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("ws-A");
  });
});
