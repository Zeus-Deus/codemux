/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";

let enableAgentChatFlag = false;
let enableLazyFlag = false;
let flagsLoaded = false;
let appStateSnapshot: unknown = null;
let homeDirSnapshot: string | null = null;

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) =>
    selector({
      enableAgentChat: enableAgentChatFlag,
      enableLazyWorkspaceCreation: enableLazyFlag,
      loaded: flagsLoaded,
    }),
  ),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(
    vi.fn((selector) =>
      selector({ appState: appStateSnapshot, homeDir: homeDirSnapshot }),
    ),
    {
      getState: () => ({
        appState: appStateSnapshot,
        homeDir: homeDirSnapshot,
      }),
    },
  ),
  useHomeDir: () => homeDirSnapshot,
}));

vi.mock("@/tauri/commands", () => ({
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
}));

import { useEnsureDraftWhenEmpty } from "./use-ensure-draft-when-empty";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { agentChatCreatePane } from "@/tauri/commands";
import {
  noteLocalActivationFallback,
  noteLocalWorkspaceActivation,
  resetLocalWorkspaceActivations,
} from "@/lib/local-activation";

const terminalSurface = {
  surface_id: "surf-1",
  root: {
    kind: "terminal",
    pane_id: "pane-t",
    session_id: "sess-1",
    title: "term",
  },
};

const emptySplitSurface = {
  surface_id: "surf-empty",
  root: {
    kind: "split",
    pane_id: "s-empty",
    direction: "horizontal",
    child_sizes: [],
    children: [],
  },
};

function resetDraftStore() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
}

describe("useEnsureDraftWhenEmpty", () => {
  beforeEach(() => {
    enableAgentChatFlag = false;
    enableLazyFlag = false;
    flagsLoaded = false;
    appStateSnapshot = null;
    homeDirSnapshot = null;
    vi.mocked(agentChatCreatePane).mockClear();
    resetDraftStore();
    // Locally-activated ids live in a module-level set for the whole
    // session; clear it so tests don't inherit each other's claims.
    resetLocalWorkspaceActivations();
  });

  it("does nothing while app state is still null", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = null;
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("does nothing while feature flags are still loading", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = false;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("does nothing when agent_chat flag is OFF", () => {
    enableAgentChatFlag = false;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("does nothing when lazy flag is OFF (legacy Home path is gone)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = false;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("does nothing when the restored workspace has a real pane (session-restore wins)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-restored",
      workspaces: [
        {
          workspace_id: "ws-restored",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("creates a Home draft when no active workspace exists", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useEnsureDraftWhenEmpty());

    const state = useChatDraftStore.getState();
    expect(state.activeHomeDraftId).not.toBeNull();
    expect(state.activeDraftId).toBe(state.activeHomeDraftId);
  });

  it("creates a Home draft when the active workspace's surface has no panes", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-restored",
      workspaces: [
        {
          workspace_id: "ws-restored",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).not.toBeNull();
  });

  it("creates a Home draft when active_workspace_id points to a missing workspace", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = {
      active_workspace_id: "ws-stale",
      workspaces: [],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).not.toBeNull();
  });

  it("does not stack duplicate drafts when the hook re-renders", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());
    const firstId = useChatDraftStore.getState().activeHomeDraftId;
    rerender();
    rerender();
    // Idempotent: the single-slot rule on the draft store means the
    // second-pass getOrCreateHomeDraft returns the same draft, so
    // activeDraftId stays stable and only one entry exists.
    expect(useChatDraftStore.getState().activeHomeDraftId).toBe(firstId);
    expect(
      Object.keys(useChatDraftStore.getState().draftsById),
    ).toHaveLength(1);
  });

  it("creates a Home draft when workspaces transition from 1 to 0 at runtime (close-all regression)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    // Start: one workspace, one real pane — no draft should fire.
    appStateSnapshot = {
      active_workspace_id: "ws-1",
      workspaces: [
        {
          workspace_id: "ws-1",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();

    // User closes the last workspace → workspaces array empties.
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    rerender();

    // Hook re-fires (no firedRef guard) and creates a Home draft,
    // so AppShell renders WorkspaceMain → DraftChatSurface instead
    // of the full-screen EmptyState.
    const state = useChatDraftStore.getState();
    expect(state.activeHomeDraftId).not.toBeNull();
    expect(state.activeDraftId).toBe(state.activeHomeDraftId);
  });

  it("does not overwrite an already-active draft when workspaces hit zero", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    // User is already on a draft (e.g., they clicked "+" then closed
    // some workspace elsewhere).
    const existing = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.getState().setActiveDraft(existing.draftId);
    appStateSnapshot = { active_workspace_id: "", workspaces: [] };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).toBe(existing.draftId);
    expect(
      Object.keys(useChatDraftStore.getState().draftsById),
    ).toHaveLength(1);
  });

  it("spawns an agent_chat pane (NOT a Home draft) when an empty project workspace is active", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    // Project workspace on master with no panes — the scenario the
    // user hit when clicking the default-branch worktree in the
    // sidebar. Without this branch the legacy Home-draft fallback
    // would paint "What should we do today?" over their workspace.
    appStateSnapshot = {
      active_workspace_id: "ws-foo-master",
      workspaces: [
        {
          workspace_id: "ws-foo-master",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    // Home draft must NOT be created for a project workspace.
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
    // Pane-spawn fired exactly once against the clicked workspace.
    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
    expect(agentChatCreatePane).toHaveBeenCalledWith(
      "ws-foo-master",
      "claude",
      null,
    );
  });

  it("creates a Home draft (legacy path) when the active empty workspace IS home-rooted", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    appStateSnapshot = {
      active_workspace_id: "ws-home",
      workspaces: [
        {
          workspace_id: "ws-home",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/home/user",
          cwd: "/home/user",
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).not.toBeNull();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("falls through to the Home-draft path when homeDir has not resolved yet (null)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = null;
    // Without a known homeDir we cannot distinguish Home from project
    // workspaces. The safer default is the legacy Home-draft path;
    // once homeDir lands, subsequent effect fires can reclassify.
    appStateSnapshot = {
      active_workspace_id: "ws-mystery",
      workspaces: [
        {
          workspace_id: "ws-mystery",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    expect(useChatDraftStore.getState().activeDraftId).not.toBeNull();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("does not double-spawn when the hook re-renders with the same empty project workspace", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    appStateSnapshot = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/foo",
          cwd: "/projects/foo",
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());
    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
    // Repeated fires while the spawn is in-flight must NOT produce
    // more Tauri calls — the in-flight ref is the guard.
    rerender();
    rerender();
    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
  });

  it("does NOT re-spawn when only an irrelevant workspace field changes (perf — fingerprint guard)", () => {
    // Regression guard for the audit-pass fix that switched this hook
    // from a `[appState]` dep array to a primitive-fingerprint
    // selector. Without the fingerprint, every backend
    // `app-state-changed` tick (agent tokens, git polls, hook events)
    // re-ran the effect body — walking surface trees and doing
    // workspace lookups for nothing. The `inFlightSpawnRef` guard
    // prevented duplicate IPC, but the wasted cycles still added up.
    //
    // This test simulates a backend tick that changes `git_branch` on
    // the active workspace (a frequent mutation under the 5 s git
    // poll). The effect's fingerprint is built from
    // `(active_workspace_id, hasPane, project_root)` — `git_branch`
    // is intentionally NOT in it — so the effect must NOT fire a
    // second time across the rerender.
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    appStateSnapshot = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/foo",
          cwd: "/projects/foo",
          git_branch: "main",
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());
    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);

    // Backend tick: same fingerprint fields, different git_branch +
    // a different workspaces array reference (Rust rebuilds the
    // snapshot on every emit). The hook's effect MUST NOT fire
    // again — fingerprint is reference-equal.
    appStateSnapshot = {
      active_workspace_id: "ws-foo",
      workspaces: [
        {
          workspace_id: "ws-foo",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/foo",
          cwd: "/projects/foo",
          git_branch: "feature-x",
        },
      ],
    };
    rerender();

    // Either branch of the proof works:
    //  - in-flight ref is set, so even if the effect DID fire it
    //    would short-circuit; OR
    //  - fingerprint is unchanged, so the effect didn't re-fire at all.
    // The contract we care about is: zero extra Tauri calls.
    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
  });

  it("falls back to workspace.cwd when project_root is missing (ad-hoc workspaces)", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    // Some workspaces (historically ad-hoc terminal workspaces) have a
    // null project_root. Treat cwd as the authoritative root when
    // deciding Home vs project.
    appStateSnapshot = {
      active_workspace_id: "ws-adhoc-home",
      workspaces: [
        {
          workspace_id: "ws-adhoc-home",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: null,
          cwd: "/home/user",
        },
      ],
    };
    renderHook(() => useEnsureDraftWhenEmpty());
    // cwd === homeDir → Home draft, no pane spawn.
    expect(useChatDraftStore.getState().activeDraftId).not.toBeNull();
    expect(agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("does not spawn a pane for an empty project workspace that became active without a local activation", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    // First render adopts whatever is active at boot. Give it a
    // workspace that already has a pane so nothing spawns here.
    appStateSnapshot = {
      active_workspace_id: "ws-mine",
      workspaces: [
        {
          workspace_id: "ws-mine",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
          project_root: "/projects/mine",
          cwd: "/projects/mine",
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());
    expect(agentChatCreatePane).not.toHaveBeenCalled();

    // Another client (desktop window, remote web client) creates a
    // workspace and switches to it. `active_workspace_id` is shared, so
    // the switch lands in OUR snapshot too — but that client is already
    // launching the pane. Spawning one from here duplicates it.
    appStateSnapshot = {
      active_workspace_id: "ws-theirs",
      workspaces: [
        {
          workspace_id: "ws-theirs",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/theirs",
          cwd: "/projects/theirs",
        },
      ],
    };
    rerender();

    expect(agentChatCreatePane).not.toHaveBeenCalled();
    // And no consolation Home draft either — the workspace isn't ours
    // to decide anything about.
    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });

  it("spawns for a post-boot workspace once THIS client records the activation", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    appStateSnapshot = {
      active_workspace_id: "ws-mine",
      workspaces: [
        {
          workspace_id: "ws-mine",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
          project_root: "/projects/mine",
          cwd: "/projects/mine",
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());

    // The user clicked this workspace in OUR window — the one activation
    // path records it before the snapshot catches up.
    noteLocalWorkspaceActivation("ws-clicked");
    appStateSnapshot = {
      active_workspace_id: "ws-clicked",
      workspaces: [
        {
          workspace_id: "ws-clicked",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/clicked",
          cwd: "/projects/clicked",
        },
      ],
    };
    rerender();

    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
    expect(agentChatCreatePane).toHaveBeenCalledWith(
      "ws-clicked",
      "claude",
      null,
    );
  });

  it("spawns for the workspace the backend fell back to after THIS client closed one", () => {
    enableAgentChatFlag = true;
    enableLazyFlag = true;
    flagsLoaded = true;
    homeDirSnapshot = "/home/user";
    appStateSnapshot = {
      active_workspace_id: "ws-mine",
      workspaces: [
        {
          workspace_id: "ws-mine",
          active_surface_id: "surf-1",
          surfaces: [terminalSurface],
          project_root: "/projects/mine",
          cwd: "/projects/mine",
        },
      ],
    };
    const { rerender } = renderHook(() => useEnsureDraftWhenEmpty());

    // Closing or archiving the active workspace never calls
    // `activateWorkspace`: the backend picks the next workspace itself.
    // The command wrapper leaves this marker so the fallback still counts
    // as ours.
    noteLocalActivationFallback();
    appStateSnapshot = {
      active_workspace_id: "ws-fallback",
      workspaces: [
        {
          workspace_id: "ws-fallback",
          active_surface_id: "surf-empty",
          surfaces: [emptySplitSurface],
          project_root: "/projects/fallback",
          cwd: "/projects/fallback",
        },
      ],
    };
    rerender();

    expect(agentChatCreatePane).toHaveBeenCalledTimes(1);
    expect(agentChatCreatePane).toHaveBeenCalledWith(
      "ws-fallback",
      "claude",
      null,
    );
  });
});
