/// <reference types="@testing-library/jest-dom/vitest" />
import { Profiler, memo } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// ── Mock the heavy leaf panes ──
//
// xterm / webview leaves cannot mount in jsdom. We stub them as
// React.memo-wrapped components that receive the SAME props the real ones
// do, so the memoization semantics of the tree ABOVE them (PaneContainer →
// PaneNode → PaneNode …, the units under test) are preserved exactly. We do
// NOT mock PaneNode or PaneContainer.
vi.mock("@/components/terminal/TerminalPane", () => ({
  TerminalPane: memo(({ sessionId }: { sessionId: string }) => (
    <div data-testid={`term-${sessionId}`} />
  )),
}));
vi.mock("@/components/browser/BrowserPane", () => ({
  BrowserPane: memo(({ browserId }: { browserId: string }) => (
    <div data-testid={`browser-${browserId}`} />
  )),
}));
vi.mock("@/components/chat/AgentChatPane", () => ({
  AgentChatPane: memo(() => <div data-testid="agent-chat" />),
}));
vi.mock("@/components/chat/AgentChatPaneHeader", () => ({
  AgentChatPaneHeader: memo(() => <div data-testid="agent-chat-header" />),
}));

// Pane action handlers call these; stub so nothing reaches Tauri IPC.
vi.mock("@/tauri/commands", () => ({
  splitPane: vi.fn(),
  closePane: vi.fn(),
  activatePane: vi.fn(),
  resizeSplit: vi.fn(),
  swapPanes: vi.fn(),
}));

import { PaneContainer } from "./pane-container";
import { useActiveWorkspace, useAppStore } from "@/stores/app-store";

// ── Snapshot fixture builders (adapted from app-store.test.ts) ──

type SplitNode = Extract<PaneNodeSnapshot, { kind: "split" }>;

function termPane(
  paneId: string,
  sessionId: string,
  title: string,
): PaneNodeSnapshot {
  return { kind: "terminal", pane_id: paneId, session_id: sessionId, title };
}

function split(...children: PaneNodeSnapshot[]): SplitNode {
  return {
    kind: "split",
    pane_id: "split-" + children.map((c) => c.pane_id).join("-"),
    direction: "horizontal",
    child_sizes: children.map(() => 1 / children.length),
    children,
  };
}

function makeSurface(
  root: PaneNodeSnapshot,
  surfaceId: string,
  activePaneId: string,
): SurfaceSnapshot {
  return { surface_id: surfaceId, title: "", root, active_pane_id: activePaneId };
}

function makeWs(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
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
    tabs: [
      {
        tab_id: "tab-1",
        kind: "terminal",
        title: "sh",
        surface_id: "",
        browser_id: null,
        icon: null,
      },
    ],
    active_tab_id: "tab-1",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
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

/** Two workspaces: A is active with a SPLIT root (two terminal children —
 *  exercises the recursive memoized PaneNode); B is inactive with a single
 *  terminal. Each call returns fresh objects so tests don't share refs. */
function makeSnapshot(): AppStateSnapshot {
  const wsA = makeWs({
    workspace_id: "ws-A",
    active_surface_id: "sf-A",
    surfaces: [
      makeSurface(
        split(
          termPane("pa1", "sa1", "Left"),
          termPane("pa2", "sa2", "Right"),
        ),
        "sf-A",
        "pa1",
      ),
    ],
  });
  const wsB = makeWs({
    workspace_id: "ws-B",
    active_surface_id: "sf-B",
    surfaces: [makeSurface(termPane("pb", "sb", "B-term"), "sf-B", "pb")],
  });
  const app = makeAppState([wsA, wsB]);
  app.active_workspace_id = "ws-A";
  return app;
}

// ── Render harness ──
//
// Reads the ACTIVE workspace through the real selector, so the test drives
// the real store → useActiveWorkspace selector → React.memo chain end to
// end. Module-level counters record the harness's own render count and the
// Profiler commit count for the pane tree.
let harnessRenders = 0;
let profilerCommits = 0;

function onRender() {
  profilerCommits++;
}

function Harness() {
  harnessRenders++;
  const ws = useActiveWorkspace();
  if (!ws) return null;
  return (
    <Profiler id="pane-tree" onRender={onRender}>
      <PaneContainer workspace={ws} />
    </Profiler>
  );
}

const setAppState = (snap: AppStateSnapshot) =>
  useAppStore.getState().setAppState(snap);

/** Seed the store with `snap` as the prev, then mount the harness. Returns
 *  the render result plus the post-mount counter baseline. */
function mount(snap: AppStateSnapshot) {
  act(() => {
    useAppStore.setState({ appState: null });
    setAppState(snap);
  });
  const utils = render(<Harness />);
  return { ...utils, baseProfiler: profilerCommits, baseHarness: harnessRenders };
}

beforeEach(() => {
  useAppStore.setState({ appState: null });
  harnessRenders = 0;
  profilerCommits = 0;
});

afterEach(() => {
  cleanup();
  useAppStore.setState({ appState: null });
});

describe("pane tree re-render economics (#127)", () => {
  it("mounts the active workspace's split pane tree", () => {
    const { getByTestId } = mount(makeSnapshot());
    // Both terminal leaves of ws-A's split root are present.
    expect(getByTestId("term-sa1")).toBeInTheDocument();
    expect(getByTestId("term-sa2")).toBeInTheDocument();
  });

  it("no-op tick (deep-equal fresh-ref snapshot) causes ZERO re-renders", () => {
    const snap = makeSnapshot();
    const { baseProfiler, baseHarness } = mount(snap);

    // Fresh refs everywhere, simulating IPC JSON deserialization.
    const clone = structuredClone(snap);
    expect(clone).not.toBe(snap);
    act(() => setAppState(clone));

    // Structural sharing returns the prev top-level ref → the selector
    // yields an identical workspace ref → the harness never re-runs and the
    // profiled pane tree never commits.
    expect(profilerCommits - baseProfiler).toBe(0);
    expect(harnessRenders - baseHarness).toBe(0);
  });

  it("a change to a DIFFERENT workspace causes ZERO active-tree re-renders", () => {
    const snap = makeSnapshot();
    const { baseProfiler, baseHarness } = mount(snap);

    // Bump inactive workspace B only. A's subtree keeps its identity.
    const clone = structuredClone(snap);
    clone.workspaces[1].git_ahead = 5;
    act(() => setAppState(clone));

    expect(profilerCommits - baseProfiler).toBe(0);
    expect(harnessRenders - baseHarness).toBe(0);
  });

  it("sanity: a real change to the active workspace DOES re-render and update the DOM", () => {
    const snap = makeSnapshot();
    const { baseProfiler, container } = mount(snap);

    // Rename the active workspace's first terminal pane.
    const clone = structuredClone(snap);
    const firstChild = (clone.workspaces[0].surfaces[0].root as SplitNode)
      .children[0] as Extract<PaneNodeSnapshot, { kind: "terminal" }>;
    firstChild.title = "Renamed";
    act(() => setAppState(clone));

    // The tree re-renders (guards against over-memoization) and the DOM
    // reflects the new title.
    expect(profilerCommits - baseProfiler).toBeGreaterThan(0);
    expect(container.querySelector('[data-pane-title="Renamed"]')).not.toBeNull();
    expect(container.querySelector('[data-pane-title="Left"]')).toBeNull();
    // The sibling pane is untouched.
    expect(container.querySelector('[data-pane-title="Right"]')).not.toBeNull();
  });

  it("sanity: replacing the split root with a single pane updates the DOM", () => {
    const snap = makeSnapshot();
    const { getByTestId, queryByTestId } = mount(snap);
    expect(getByTestId("term-sa1")).toBeInTheDocument();
    expect(getByTestId("term-sa2")).toBeInTheDocument();

    // Collapse the split down to a single terminal.
    const clone = structuredClone(snap);
    const surface = clone.workspaces[0].surfaces[0];
    surface.root = termPane("pa-solo", "sa-solo", "Solo");
    surface.active_pane_id = "pa-solo";
    act(() => setAppState(clone));

    // One child is gone; the new single pane is mounted.
    expect(queryByTestId("term-sa1")).toBeNull();
    expect(queryByTestId("term-sa2")).toBeNull();
    expect(getByTestId("term-sa-solo")).toBeInTheDocument();
  });
});
