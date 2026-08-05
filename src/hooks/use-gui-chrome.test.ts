/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";

import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore, type DraftId } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

import { useAgentChatPaneActive, useGuiChrome } from "./use-gui-chrome";

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/path/to/project",
    git_branch: "main",
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
    ...overrides,
  };
}

function setActiveWorkspace(overrides: Partial<WorkspaceSnapshot> = {}) {
  const ws = makeWorkspace(overrides);
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: ws.workspace_id,
      workspaces: [ws],
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
        stores_live_process_state: false,
      },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

// This is the single source of truth for the GUI chrome predicate
// (docs/features/gui-chrome.md): title-bar.tsx, the background-browser
// chip/context-bar indicator, and the peek overlay all key off it.
describe("useGuiChrome", () => {
  afterEach(() => {
    useAppStore.setState({ appState: null });
    useFeatureFlags.setState({
      enableAgentChat: false,
      enableLazyWorkspaceCreation: false,
    });
    useChatDraftStore.setState({ activeDraftId: null });
  });

  it("is false with no active workspace even when the flag is on", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useGuiChrome());
    expect(result.current).toBe(false);
  });

  it("is false when the Agent Chat beta flag is off", () => {
    setActiveWorkspace();
    useFeatureFlags.setState({ enableAgentChat: false });
    const { result } = renderHook(() => useGuiChrome());
    expect(result.current).toBe(false);
  });

  it("is true for a real workspace with the flag on", () => {
    setActiveWorkspace();
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useGuiChrome());
    expect(result.current).toBe(true);
  });

  it("is false while a lazy-creation chat draft is active", () => {
    setActiveWorkspace();
    useFeatureFlags.setState({
      enableAgentChat: true,
      enableLazyWorkspaceCreation: true,
    });
    useChatDraftStore.setState({ activeDraftId: "draft-1" as DraftId });
    const { result } = renderHook(() => useGuiChrome());
    expect(result.current).toBe(false);
  });
});

// Drives `WorkspaceContextBar`'s hide rule — see
// docs/features/workspace-context-bar.md. Builds a real
// surfaces/pane-tree snapshot (rather than mocking) so the recursive
// split-tree walk is exercised for real.
describe("useAgentChatPaneActive", () => {
  afterEach(() => {
    useAppStore.setState({ appState: null });
    useFeatureFlags.setState({
      enableAgentChat: false,
      enableLazyWorkspaceCreation: false,
    });
    useChatDraftStore.setState({ activeDraftId: null });
  });

  function setActiveWorkspaceWithPane(root: PaneNodeSnapshot, activePaneId: string) {
    setActiveWorkspace({
      active_surface_id: "surf-1",
      surfaces: [{ surface_id: "surf-1", title: "Main", root, active_pane_id: activePaneId }],
    });
  }

  it("is false with no active workspace even when the flag is on", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useAgentChatPaneActive());
    expect(result.current).toBe(false);
  });

  it("is true when the active pane of the active surface is an agent_chat pane", () => {
    setActiveWorkspaceWithPane(
      { kind: "agent_chat", pane_id: "pane-1", title: "Chat", thread_id: null, provider: null, cwd: null },
      "pane-1",
    );
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useAgentChatPaneActive());
    expect(result.current).toBe(true);
  });

  it("is false when the active pane is a terminal — the bottom bar stays", () => {
    setActiveWorkspaceWithPane(
      { kind: "terminal", pane_id: "pane-1", session_id: "sess-1", title: "Shell" },
      "pane-1",
    );
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useAgentChatPaneActive());
    expect(result.current).toBe(false);
  });

  it("resolves the active pane through nested splits", () => {
    setActiveWorkspaceWithPane(
      {
        kind: "split",
        pane_id: "split-1",
        direction: "horizontal",
        child_sizes: [0.5, 0.5],
        children: [
          { kind: "terminal", pane_id: "pane-1", session_id: "sess-1", title: "Shell" },
          {
            kind: "split",
            pane_id: "split-2",
            direction: "vertical",
            child_sizes: [0.5, 0.5],
            children: [
              { kind: "terminal", pane_id: "pane-2", session_id: "sess-2", title: "Shell" },
              { kind: "agent_chat", pane_id: "pane-3", title: "Chat", thread_id: null, provider: null, cwd: null },
            ],
          },
        ],
      },
      "pane-3",
    );
    useFeatureFlags.setState({ enableAgentChat: true });
    const { result } = renderHook(() => useAgentChatPaneActive());
    expect(result.current).toBe(true);
  });

  it("is false when GUI chrome is off (legacy mode) even with an active agent_chat pane", () => {
    setActiveWorkspaceWithPane(
      { kind: "agent_chat", pane_id: "pane-1", title: "Chat", thread_id: null, provider: null, cwd: null },
      "pane-1",
    );
    useFeatureFlags.setState({ enableAgentChat: false });
    const { result } = renderHook(() => useAgentChatPaneActive());
    expect(result.current).toBe(false);
  });

});
