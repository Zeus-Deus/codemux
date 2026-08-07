/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkflowRunItem } from "@/lib/agent-chat/types";
import type { TasksSnapshot } from "@/tauri/events";
import type {
  AgentBrowserSession,
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import {
  DEFAULT_RIGHT_PANEL_PANES,
  RIGHT_PANEL_EMPTY,
  useUIStore,
} from "@/stores/ui-store";

// ── Mocks ──
// Sub-panels are irrelevant to deck wiring — stub them out.
vi.mock("@/components/workspace/file-tree-panel", () => ({
  FileTreePanel: () => <div data-testid="file-tree-panel" />,
}));
vi.mock("@/components/workspace/changes-panel", () => ({
  ChangesPanel: () => <div data-testid="changes-panel" />,
}));
vi.mock("@/components/workspace/review-panel", () => ({
  ReviewPanel: () => <div data-testid="review-panel" />,
}));
vi.mock("@/components/workflow/orchestration-panel", () => ({
  OrchestrationPanel: () => <div data-testid="orchestration-panel-stub" />,
}));
vi.mock("@/components/diff/DiffPane", () => ({
  DiffPane: () => <div data-testid="diff-pane-stub" />,
}));
// The browser pane's body is a WebSocket screencast onto a canvas — not
// jsdom-testable. Stub it to a sentinel so the deck wiring (which session
// the pane is mounted against) is what's asserted.
vi.mock("@/components/browser/BrowserPane", () => ({
  BrowserPane: (props: Record<string, unknown>) => (
    <div
      data-testid="browser-pane-stub"
      data-browser-id={props.browserId as string}
      data-workspace-id={props.workspaceId as string}
    />
  ),
}));

const mocks = vi.hoisted(() => ({
  workflow: { run: null as WorkflowRunItem | null, threadId: null as string | null },
  tasks: null as TasksSnapshot | null,
  // Whether the focused chat thread is mid-run. Feeds the status foot's
  // "agents working" count.
  tasksStreaming: true,
  // `titlebarOverlay` = "TitleBar renders the floating overlay, not the
  // in-flow legacy h-9 bar"; `remote` = the web remote client, which has no
  // native window controls and (therefore) no overlay drag layer either.
  titlebarOverlay: true,
  remote: false,
  dock: vi.fn().mockResolvedValue(undefined),
  undock: vi.fn().mockResolvedValue(undefined),
  createTab: vi.fn().mockResolvedValue("tab-1"),
}));
vi.mock("@/components/workflow/use-workspace-workflow", () => ({
  useWorkspaceWorkflow: () => mocks.workflow,
}));
vi.mock("@/hooks/use-active-chat-tasks", () => ({
  useActiveChatTasks: () => ({
    threadId: "thread-1",
    tasks: mocks.tasks,
    streaming: mocks.tasksStreaming,
  }),
}));
vi.mock("@/hooks/use-gui-chrome", () => ({
  useTitlebarOverlay: () => mocks.titlebarOverlay,
}));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => mocks.remote,
}));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  dockBrowserInRightPanel: (...a: unknown[]) => mocks.dock(...a),
  undockBrowserFromRightPanel: (...a: unknown[]) => mocks.undock(...a),
  createTab: (...a: unknown[]) => mocks.createTab(...a),
}));

import { RightPanel } from "./right-panel";

function chatPane(threadId: string | null): PaneNodeSnapshot {
  return {
    kind: "agent_chat",
    pane_id: "pane-chat",
    title: "Agent Chat",
    thread_id: threadId,
    provider: "claude",
    cwd: "/p",
  };
}

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  const root = chatPane("thread-1");
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [
      { tab_id: "tab-chat", kind: "terminal", title: "Agent Chat", surface_id: "surface-1", browser_id: null, icon: null },
    ],
    active_tab_id: "tab-chat",
    active_surface_id: "surface-1",
    surfaces: [{ surface_id: "surface-1", title: "s", active_pane_id: root.pane_id, root }],
    ...overrides,
  };
}

function makeRun(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-item-1",
    seq: 0,
    workflowId: "wf-1",
    status: "running",
    name: "Audit route auth",
    description: null,
    script: null,
    plannedPhases: [],
    phases: [],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: 0,
    durationMs: null,
    approvalRequestId: null,
    ...overrides,
  };
}

function renderDeck(
  props: Partial<React.ComponentProps<typeof RightPanel>> = {},
) {
  return render(
    <TooltipProvider>
      <RightPanel
        workspace={props.workspace ?? makeWorkspace()}
        activeTab={props.activeTab ?? "files"}
      />
    </TooltipProvider>,
  );
}

function makeBrowserSession(
  overrides: Partial<AgentBrowserSession> = {},
): AgentBrowserSession {
  return {
    session_id: "agent-browser-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-demo-abc123",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com/docs",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
    right_panel_docked: false,
    ...overrides,
  };
}

/** Put the browser tab in the deck, the way opening it from `+` would. */
function openBrowserPane() {
  useUIStore.setState({
    rightPanelPanes: { "ws-1": [...DEFAULT_RIGHT_PANEL_PANES, "browser"] },
    rightPanelTabs: { "ws-1": "browser" },
  });
}

function seedBrowserSession(session: AgentBrowserSession | null) {
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: "ws-1",
      workspaces: [],
      terminal_sessions: [],
      browser_sessions: [],
      agent_browser_sessions: session ? [session] : [],
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

beforeEach(() => {
  useUIStore.setState({
    rightPanelTabs: {},
    rightPanelPanes: {},
    rightPanelDismissedPanes: {},
    rightPanelWidth: 320,
  });
  mocks.dock.mockClear();
  mocks.undock.mockClear();
  useAppStore.setState({ appState: null });
});

afterEach(() => {
  cleanup();
  mocks.workflow = { run: null, threadId: null };
  mocks.tasks = null;
  mocks.tasksStreaming = true;
  mocks.titlebarOverlay = true;
  mocks.remote = false;
  mocks.createTab.mockClear();
});

// The tab row used to reserve a blank `mt-10` strip for the floating
// titlebar and start below it: a 40px empty header over the panel, with the
// panel toggle stranded at its far left. The row now *is* that band — 40px
// tall, flush with the window's top edge, padded only on the right for the
// fixed cluster and the native window buttons above it.
describe("RightPanel titlebar band", () => {
  it("renders the tab row as the window band, with no blank strip above it", () => {
    renderDeck();
    const header = screen.getByTestId("right-panel-tabs-header");
    expect(header).not.toHaveClass("mt-10");
    expect(header).toHaveClass("h-10");
    expect(header).toHaveAttribute("data-in-titlebar", "true");
    // Clears the fixed panel cluster (56 + 6) and the window buttons (104).
    expect(header.style.paddingRight).toBe("166px");
  });

  // The titlebar is frameless: no cluster paints a surface of its own, and
  // the sidebar / workspace / panel all reach the physical top edge. A
  // `bg-card` fill here drew a lighter grey slab across the panel's half of
  // the band, with a visible seam where the workspace's half ended.
  it("paints no surface of its own, so the band reads as one window edge", () => {
    renderDeck();
    const header = screen.getByTestId("right-panel-tabs-header");
    expect(header).toHaveClass("bg-transparent");
    expect(header).not.toHaveClass("bg-card");
    // One hairline under the row, and nothing above or behind the tabs.
    expect(header).toHaveClass("border-b");
  });

  it("keeps the card fill under the legacy in-flow bar", () => {
    mocks.titlebarOverlay = false;
    renderDeck();
    expect(screen.getByTestId("right-panel-tabs-header")).toHaveClass("bg-card");
  });

  // The other end of the same surface. The foot stays — it carries the pane's
  // status line — but a filled one was the last slab breaking the panel into
  // stripes. Unconditional, because unlike the tab row it never abuts window
  // chrome that it would need to match.
  it("gives the status foot a hairline and no fill, in either chrome mode", () => {
    renderDeck();
    const foot = screen.getByTestId("right-panel-status-foot");
    expect(foot).toHaveClass("bg-transparent", "border-t");
    expect(foot).not.toHaveClass("bg-card");

    cleanup();
    mocks.titlebarOverlay = false;
    renderDeck();
    expect(screen.getByTestId("right-panel-status-foot")).toHaveClass(
      "bg-transparent",
    );
  });

  // The web client renders no window controls, so the reserve shrinks to the
  // cluster alone. Safe only because the overlay also drops its drag layer
  // there — see the `titlebar-drag-layer` coverage in `title-bar.test.tsx`.
  it("reserves only the cluster on the web client, which has no window buttons", () => {
    mocks.remote = true;
    renderDeck();
    expect(
      screen.getByTestId("right-panel-tabs-header").style.paddingRight,
    ).toBe("68px");
  });

  // With the GUI flag off the in-flow legacy `h-9` bar already occupies the
  // top of the window, so the panel's row is an ordinary 36px strip below it
  // and keeps the panel controls it can't delegate to a cluster that isn't
  // rendered.
  it("stays an ordinary 36px row under the in-flow legacy bar", () => {
    mocks.titlebarOverlay = false;
    renderDeck();
    const header = screen.getByTestId("right-panel-tabs-header");
    expect(header).toHaveClass("h-9");
    expect(header).not.toHaveAttribute("data-in-titlebar");
    expect(header.style.paddingRight).toBe("");
    expect(
      header.contains(screen.getByRole("button", { name: "Close panel" })),
    ).toBe(true);
  });

  // The titlebar's own drag layer stops at the panel's left edge so it can't
  // swallow these tabs, which makes the gap after them the panel's only
  // window-drag surface.
  it("keeps a drag surface in the row on desktop and drops it on the web", () => {
    renderDeck();
    expect(screen.getByTestId("right-panel-drag-gap")).toHaveAttribute(
      "data-tauri-drag-region",
    );
    cleanup();
    mocks.remote = true;
    renderDeck();
    expect(screen.getByTestId("right-panel-drag-gap")).not.toHaveAttribute(
      "data-tauri-drag-region",
    );
  });
});

describe("RightPanel deck", () => {
  it("opens with the default deck and one row of chrome plus a status foot", () => {
    renderDeck();
    expect(screen.getByTestId("changes-tab")).toBeInTheDocument();
    expect(screen.getByTestId("review-tab")).toBeInTheDocument();
    // One band of chrome for the whole deck, not one per pane, and no
    // second breadcrumb row under it.
    expect(screen.getAllByTestId("right-panel-tabs-header")).toHaveLength(1);
    expect(screen.queryByTestId("right-panel-pane-bar")).toBeNull();
    expect(screen.queryByTestId("right-panel-crumb")).toBeNull();
    expect(screen.getAllByTestId("right-panel-status-foot")).toHaveLength(1);
  });

  // The point of the merge: the panel used to stack the window tab bar, its
  // own tab strip and a "<workspace> › <pane>" breadcrumb before the first
  // line of content. The tabs and the pane's controls share one row.
  it("renders the active pane's actions inside the tab row, not a second band", () => {
    renderDeck({ activeTab: "files" });
    const row = screen.getByTestId("right-panel-tabs-header");
    expect(row.contains(screen.getByTestId("files-refresh"))).toBe(true);
    expect(row.contains(screen.getByTestId("files-hidden-toggle"))).toBe(true);
    // The panel-level controls are *not* here in GUI chrome — they belong to
    // the titlebar's fixed cluster, which is what stops them sliding across
    // the window every time the panel opens.
    expect(screen.queryByRole("button", { name: "Close panel" })).toBeNull();
    // Nothing repeats the workspace name next to the pane name.
    expect(row).not.toHaveTextContent("demo ›");
  });

  // Actions swap in place, so the slot has to belong to the active pane and
  // nobody else — a stale Refresh from the tree while Changes is on screen
  // is exactly the confusion the merge was meant to remove.
  it("swaps the action slot when the active pane changes", () => {
    const workspace = makeWorkspace({ git_additions: 130, git_deletions: 12 });
    const view = renderDeck({ workspace, activeTab: "files" });
    expect(screen.getByTestId("files-refresh")).toBeInTheDocument();
    expect(screen.queryByTestId("changes-filter")).toBeNull();

    view.rerender(
      <TooltipProvider>
        <RightPanel workspace={workspace} activeTab="changes" />
      </TooltipProvider>,
    );

    const row = screen.getByTestId("right-panel-tabs-header");
    expect(screen.queryByTestId("files-refresh")).toBeNull();
    expect(row.contains(screen.getByTestId("changes-filter"))).toBe(true);
    // The +N/−N totals ride in the same slot.
    expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
      "+130",
    );
    expect(screen.getByTestId("right-panel-pane-actions")).toHaveTextContent(
      "−12",
    );
  });

  // Tabs carry their state as a fill, not a box. A border on the active tab
  // was the single heaviest thing in the panel.
  it("keeps the tabs light — filled active tab, faded inactive, no border", () => {
    renderDeck({ activeTab: "files" });
    const changes = screen.getByTestId("changes-tab");
    expect(changes).toHaveAttribute("data-state", "inactive");
    expect(changes).toHaveClass("h-[26px]", "text-foreground/42", "font-medium");
    expect(changes.className).not.toMatch(/(^|\s)border/);

    const review = screen.getByTestId("review-tab");
    expect(review.className).not.toMatch(/(^|\s)(border|shadow)/);
  });

  it("closes a core pane and offers it again from the + menu", async () => {
    const user = userEvent.setup();
    renderDeck();
    await user.click(screen.getByRole("button", { name: "Close Changes" }));

    expect(screen.queryByTestId("changes-tab")).toBeNull();
    expect(useUIStore.getState().rightPanelPanes["ws-1"]).not.toContain("changes");

    await user.click(screen.getByTestId("right-panel-add-pane"));
    await user.click(await screen.findByRole("menuitem", { name: "Changes" }));
    expect(screen.getByTestId("changes-tab")).toBeInTheDocument();
  });

  // Closing the pane you're looking at must land somewhere, not on a blank
  // body — the neighbour takes focus.
  it("hands focus to a neighbour when the active pane is closed", async () => {
    const user = userEvent.setup();
    renderDeck({ activeTab: "files" });
    await user.click(screen.getByRole("button", { name: "Close Files" }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
  });

  // `null` has always meant "panel collapsed"; the last close keeps that
  // contract instead of leaving an empty strip stranded on screen.
  // Closing a tab and dismissing the column it lives in are different
  // requests. The last close lands on the surface picker; the titlebar's
  // panel toggle is still the way to collapse the panel entirely.
  it("lands on the surface picker when the last pane is closed", async () => {
    const user = userEvent.setup();
    useUIStore.setState({ rightPanelPanes: { "ws-1": ["files"] } });
    renderDeck({ activeTab: "files" });
    await user.click(screen.getByRole("button", { name: "Close Files" }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe(RIGHT_PANEL_EMPTY);
  });

  it("offers every openable surface as a card when the deck is empty", async () => {
    const user = userEvent.setup();
    useUIStore.setState({ rightPanelPanes: { "ws-1": [] } });
    renderDeck({ activeTab: RIGHT_PANEL_EMPTY });

    const picker = screen.getByTestId("right-panel-picker");
    expect(picker).toHaveTextContent("Open a surface");
    expect(picker).toHaveTextContent("Choose what to show in the right panel.");
    // Every card carries its one-line description from the registry.
    expect(picker).toHaveTextContent("Browse and read the workspace tree.");

    await user.click(screen.getByTestId("right-panel-picker-changes"));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toContain("changes");
  });

  // Terminal is a *workspace* pane, so its card must route to the same
  // backend action the `+` menu's Terminal item uses rather than inventing a
  // deck pane that doesn't exist.
  it("routes the picker's Terminal card to a real workspace terminal", async () => {
    const user = userEvent.setup();
    useUIStore.setState({ rightPanelPanes: { "ws-1": [] } });
    renderDeck({ activeTab: RIGHT_PANEL_EMPTY });

    await user.click(screen.getByTestId("right-panel-picker-terminal"));
    expect(mocks.createTab).toHaveBeenCalledWith("ws-1", "terminal");
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toEqual([]);
  });

  it("routes the + menu's Open file… to the file-search dialog, targeted at the deck", async () => {
    const user = userEvent.setup();
    renderDeck();
    await user.click(screen.getByTestId("right-panel-add-pane"));
    await user.click(await screen.findByRole("menuitem", { name: /Open file/ }));
    expect(useUIStore.getState().showFileSearch).toBe(true);
    expect(useUIStore.getState().fileSearchTarget).toBe("right-panel");
  });

  it("keeps the default deck reachable after a workspace has never been touched", () => {
    renderDeck();
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toEqual([
      ...DEFAULT_RIGHT_PANEL_PANES,
    ]);
  });
});

describe("RightPanel Tasks pane", () => {
  it("is absent until the focused chat has tasks", () => {
    renderDeck();
    expect(screen.queryByTestId("tasks-tab")).toBeNull();
  });

  it("auto-opens with a progress badge and renders the task body", () => {
    mocks.tasks = {
      tasks: [
        { task_id: "1", title: "Done", status: "completed", blocked_by: [] },
        { task_id: "2", title: "Working", status: "in_progress", blocked_by: [] },
      ],
    };
    renderDeck({ activeTab: "tasks" });
    expect(screen.getByTestId("tasks-tab")).toHaveTextContent("1/2");
    expect(screen.getByTestId("tasks-panel")).toBeInTheDocument();
  });

  // The strip's live affordance is the badge, not a blinking dot: the
  // count is tinted with the theme accent while the pane is the one
  // you're looking at. Nothing in the strip animates any more.
  it("tints the tasks badge with the accent only while its pane is active", () => {
    mocks.tasks = {
      tasks: [{ task_id: "1", title: "Working", status: "in_progress", blocked_by: [] }],
    };
    const { rerender } = renderDeck({ activeTab: "tasks" });
    expect(
      screen.getByTestId("tasks-tab").querySelector(".text-accent-ember"),
    ).not.toBeNull();

    rerender(
      <TooltipProvider>
        <RightPanel workspace={makeWorkspace()} activeTab="files" />
      </TooltipProvider>,
    );
    expect(
      screen.getByTestId("tasks-tab").querySelector(".text-accent-ember"),
    ).toBeNull();
  });

  it("carries no blinking dots anywhere in the strip", () => {
    mocks.tasks = {
      tasks: [{ task_id: "1", title: "Working", status: "in_progress", blocked_by: [] }],
    };
    renderDeck({ activeTab: "files" });
    expect(
      screen.getByTestId("right-panel-tabs-header").querySelector(".cm-blink"),
    ).toBeNull();
  });

  // The tasks pane's Copy button used to be a footer inside the panel; it
  // is a pane action in the deck's one row now. The "updates live" caption
  // that travelled with it is gone — the status foot below reports the
  // live figure it was promising.
  it("puts the tasks pane's Copy action in the tab row, without the caption", () => {
    mocks.tasks = {
      tasks: [{ task_id: "1", title: "Working", status: "in_progress", blocked_by: [] }],
    };
    renderDeck({ activeTab: "tasks" });
    const row = screen.getByTestId("right-panel-tabs-header");
    expect(row.contains(screen.getByTestId("tasks-copy"))).toBe(true);
    expect(row).not.toHaveTextContent("updates live");
    expect(screen.getByTestId("right-panel-status-foot")).toHaveTextContent(
      "0 of 1 done · 1 working",
    );
  });
});

describe("RightPanel Orchestration pane", () => {
  it("hides the Orchestration pane when the workspace has no workflow run", () => {
    mocks.workflow = { run: null, threadId: null };
    renderDeck();
    expect(screen.queryByTestId("orchestration-tab")).toBeNull();
  });

  it("hides the Orchestration pane while the run is pending approval", () => {
    // The in-thread approval card owns the pending_approval state; the
    // panel only appears once the run is approved (design mock: the
    // approval state renders no side panel).
    mocks.workflow = {
      run: makeRun({ status: "pending_approval", approvalRequestId: "req-1" }),
      threadId: "thread-1",
    };
    renderDeck();
    expect(screen.queryByTestId("orchestration-tab")).toBeNull();
  });

  it("auto-opens the Orchestration pane once a run is approved", () => {
    mocks.workflow = { run: makeRun({ status: "running" }), threadId: "thread-1" };
    renderDeck();
    expect(screen.getByTestId("orchestration-tab")).toBeInTheDocument();
  });

  it("renders the OrchestrationPanel body when the orchestration pane is active", () => {
    mocks.workflow = { run: makeRun({ status: "running" }), threadId: "thread-1" };
    renderDeck({ activeTab: "orchestration" });
    expect(screen.getByTestId("orchestration-panel-stub")).toBeInTheDocument();
  });

  // Once closed, an availability-gated pane must stay closed — otherwise
  // the auto-open effect would put it straight back on the next render.
  it("does not re-open a conditional pane the user closed", async () => {
    const user = userEvent.setup();
    mocks.workflow = { run: makeRun({ status: "running" }), threadId: "thread-1" };
    renderDeck();
    await user.click(screen.getByRole("button", { name: "Close Orchestration" }));
    expect(screen.queryByTestId("orchestration-tab")).toBeNull();
  });
});

// The browser is a deck pane now, not a jump-out that splits the main area.
// What makes it correct is identity: the pane must mount against the
// workspace's ONE agent browser session (`cli_session_name` — the daemon key
// every `codemux browser` call resolves to), never a browser of its own.
describe("RightPanel browser pane", () => {
  it("offers Browser in the + menu and docks the workspace session on open", async () => {
    seedBrowserSession(makeBrowserSession());
    renderDeck();

    await userEvent.click(screen.getByTestId("right-panel-add-pane"));
    await userEvent.click(screen.getByRole("menuitem", { name: "Browser" }));

    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBe("browser");
    expect(mocks.dock).toHaveBeenCalledWith("ws-1");
  });

  it("mounts the pane against the session's cli_session_name once docked", () => {
    seedBrowserSession(makeBrowserSession({ right_panel_docked: true }));
    openBrowserPane();
    renderDeck({ activeTab: "browser" });

    const stub = screen.getByTestId("browser-pane-stub");
    expect(stub).toHaveAttribute("data-browser-id", "ws-demo-abc123");
    expect(stub).toHaveAttribute("data-workspace-id", "ws-1");
    // Nav controls sit in the tab row's action slot; the address is too
    // long to share that row honestly, so it lives in the status foot
    // (scheme stripped) instead of a second band.
    const row = screen.getByTestId("right-panel-tabs-header");
    expect(row.contains(screen.getByTestId("browser-pane-reload"))).toBe(true);
    expect(row).not.toHaveTextContent("example.com/docs");
    expect(screen.getByTestId("right-panel-status-foot")).toHaveTextContent(
      "example.com/docs",
    );
  });

  it("waits for the backend rather than guessing a session name", () => {
    seedBrowserSession(makeBrowserSession({ right_panel_docked: false }));
    openBrowserPane();
    renderDeck({ activeTab: "browser" });

    expect(screen.getByTestId("browser-pane-connecting")).toBeInTheDocument();
    expect(screen.queryByTestId("browser-pane-stub")).not.toBeInTheDocument();
  });

  it("undocks as an explicit dismissal when the tab is closed", async () => {
    seedBrowserSession(makeBrowserSession({ right_panel_docked: true }));
    openBrowserPane();
    renderDeck({ activeTab: "browser" });

    await userEvent.click(screen.getByRole("button", { name: "Close Browser" }));

    expect(mocks.undock).toHaveBeenCalledWith("ws-1", true);
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).not.toContain(
      "browser",
    );
  });

  // A collapsed panel is not a surface — leaving the session docked would
  // hide the browser from the agent's pane gate AND from the background chip.
  // Legacy chrome keeps this button in the panel's own row; in GUI chrome the
  // titlebar cluster owns it and carries the same rule (see
  // `title-bar.test.tsx`).
  it("undocks without dismissing when the whole panel is collapsed", async () => {
    mocks.titlebarOverlay = false;
    seedBrowserSession(makeBrowserSession({ right_panel_docked: true }));
    openBrowserPane();
    renderDeck({ activeTab: "browser" });

    await userEvent.click(screen.getByRole("button", { name: "Close panel" }));

    expect(mocks.undock).toHaveBeenCalledWith("ws-1", false);
    // The tab is still in the deck, so re-opening the panel brings it back.
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toContain("browser");
  });

  it("yields its tab when the session moves into a main-area pane", async () => {
    seedBrowserSession(makeBrowserSession({ right_panel_docked: true }));
    openBrowserPane();
    const view = renderDeck({ activeTab: "browser" });
    expect(screen.getByTestId("browser-pane-stub")).toBeInTheDocument();

    // Someone opened a browser from the workspace tab strip, which
    // re-attaches this session to a pane-tree node. One session, one
    // surface — the deck must let go rather than mirror it.
    seedBrowserSession(
      makeBrowserSession({ right_panel_docked: false, pane_id: "pane-9" }),
    );
    view.rerender(
      <TooltipProvider>
        <RightPanel workspace={makeWorkspace()} activeTab="browser" />
      </TooltipProvider>,
    );

    await vi.waitFor(() => {
      expect(useUIStore.getState().getRightPanelPanes("ws-1")).not.toContain(
        "browser",
      );
    });
    // Crucially it did NOT re-dock and fight the pane for the session.
    expect(mocks.dock).not.toHaveBeenCalled();
  });
});
