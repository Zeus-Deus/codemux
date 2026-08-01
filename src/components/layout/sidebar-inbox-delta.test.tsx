/// <reference types="@testing-library/jest-dom/vitest" />
/**
 * Exit gate for Phase 6 of `docs/plans/gui-responsiveness.md`: a metadata
 * change touches only its domain's subscribers.
 *
 * Unlike `sidebar-inbox.test.tsx`, this file uses the REAL app store — the
 * whole point is the path from `applyAppStateDelta` through the store's
 * narrow selectors to the memo boundary on the cards. A mocked store would
 * assert nothing about that path.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { memo } from "react";
import { render, act, cleanup } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  AppStateSnapshot,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  archiveWorkspace: vi.fn().mockResolvedValue("archive-1"),
  unarchiveWorkspace: vi.fn().mockResolvedValue("ws-1"),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  setWorkspaceMuted: vi.fn().mockResolvedValue(undefined),
  detectEditors: vi.fn().mockResolvedValue([]),
  getDefaultBranch: vi.fn().mockResolvedValue("main"),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  runWorkspaceSetup: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  revealInFileManager: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  getGithubIssue: vi.fn().mockResolvedValue(null),
  hostsList: vi.fn().mockResolvedValue([]),
  getAppState: vi.fn(() => new Promise<never>(() => {})),
  workspacePushToHost: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", remote_path: null, rsync_summary: null }),
  workspacePullBack: vi.fn().mockResolvedValue({ ok: true, message: "", rsync_summary: null }),
}));

vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: vi.fn() }),
}));

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

// Render probe. `memo` with the default shallow comparison is exactly the
// boundary `SidebarInboxCard` itself declares, so a count that stays put means
// every prop the parent handed this card was reference-equal — which is the
// property under test.
const cardRenders: Record<string, number> = {};
vi.mock("./sidebar-inbox-card", async (importOriginal) => {
  const mod = await importOriginal<typeof import("./sidebar-inbox-card")>();
  const Real = mod.SidebarInboxCard;
  const Probe = memo(function SidebarInboxCardProbe(
    props: Parameters<typeof Real>[0],
  ) {
    const id = props.workspace.workspace_id;
    cardRenders[id] = (cardRenders[id] ?? 0) + 1;
    return <Real {...props} />;
  });
  return { ...mod, SidebarInboxCard: Probe };
});

import { SidebarInbox } from "./sidebar-inbox";
import { useAppStore } from "@/stores/app-store";
import { __resetSidebarInboxStoreForTests } from "@/stores/sidebar-inbox-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";

function surfaceWithPane(paneId: string): SurfaceSnapshot[] {
  return [{ root: { kind: "pane", pane_id: paneId } }] as unknown as SurfaceSnapshot[];
}

function makeWorkspace(id: string): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: `Workspace ${id}`,
    workspace_type: "standard",
    cwd: "/home/u/projects/myapp",
    project_root: "/home/u/projects/myapp",
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
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    last_active_at: null,
    last_visited_at: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: surfaceWithPane(`pane-${id}`),
  } as unknown as WorkspaceSnapshot;
}

function makeAppState(
  workspaces: WorkspaceSnapshot[],
  revision: number,
): AppStateSnapshot {
  return {
    schema_version: 1,
    snapshot_revision: revision,
    active_workspace_id: workspaces[0]?.workspace_id ?? "",
    workspaces,
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    archived_workspaces: [],
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

function resetAppStore(): void {
  useAppStore.setState({
    appState: null,
    pendingActiveWorkspaceId: null,
    pendingActivationAt: null,
    lastSeenRevision: 0,
    resyncInFlight: false,
    resyncRequestId: 0,
  });
}

async function flush(): Promise<void> {
  for (let i = 0; i < 3; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

const IDS = ["ws-1", "ws-2", "ws-3"];

async function mountInbox() {
  useAppStore
    .getState()
    .setAppState(makeAppState(IDS.map(makeWorkspace), 100));
  render(
    <TooltipProvider>
      <SidebarInbox />
    </TooltipProvider>,
  );
  await flush();
  for (const id of IDS) delete cardRenders[id];
}

beforeEach(() => {
  vi.clearAllMocks();
  __resetSidebarInboxStoreForTests();
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
  });
  for (const id of Object.keys(cardRenders)) delete cardRenders[id];
  resetAppStore();
});

afterEach(() => {
  cleanup();
  resetAppStore();
});

describe("SidebarInbox — domain deltas touch only their own subscribers", () => {
  it("re-renders only the card whose git metadata moved", async () => {
    await mountInbox();

    await act(async () => {
      useAppStore.getState().applyAppStateDelta(101, {
        domain: "workspace_git",
        workspace_id: "ws-2",
        git: {
          is_git: true,
          git_branch: "feature",
          git_ahead: 3,
          git_behind: 0,
          git_additions: 0,
          git_deletions: 0,
          git_changed_files: 0,
        },
      });
    });
    await flush();

    expect(cardRenders["ws-2"]).toBeGreaterThan(0);
    expect(cardRenders["ws-1"] ?? 0).toBe(0);
    expect(cardRenders["ws-3"] ?? 0).toBe(0);
  });

  it("leaves every card alone for a domain no card reads", async () => {
    await mountInbox();

    await act(async () => {
      useAppStore.getState().applyAppStateDelta(101, {
        domain: "detected_ports",
        ports: [
          {
            port: 5173,
            pid: 42,
            process_name: "vite",
            workspace_id: null,
            label: null,
            source: null,
          },
        ],
      });
    });
    await flush();

    for (const id of IDS) expect(cardRenders[id] ?? 0).toBe(0);
  });

  it("re-renders only the card owning the pane whose status changed", async () => {
    await mountInbox();

    await act(async () => {
      useAppStore.getState().applyAppStateDelta(101, {
        domain: "pane_status",
        pane_id: "pane-ws-3",
        status: "working",
      });
    });
    await flush();

    expect(cardRenders["ws-3"]).toBeGreaterThan(0);
    expect(cardRenders["ws-1"] ?? 0).toBe(0);
    expect(cardRenders["ws-2"] ?? 0).toBe(0);
  });

  it("keeps every card memoized across a no-op full snapshot", async () => {
    // Structural sharing means an unchanged snapshot reconciles to the same
    // references, so the memo boundary holds for the full-snapshot path too.
    await mountInbox();

    await act(async () => {
      useAppStore
        .getState()
        .setAppState(makeAppState(IDS.map(makeWorkspace), 101));
    });
    await flush();

    for (const id of IDS) expect(cardRenders[id] ?? 0).toBe(0);
  });
});
