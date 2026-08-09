/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";

import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore, type DraftId } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type {
  AppStateSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

import { useGuiChrome } from "./use-gui-chrome";

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

// This is the single source of truth for the GUI chrome predicate:
// title-bar.tsx, the background-browser
// chip/terminal-header indicator, and the peek overlay all key off it.
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
