/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type {
  WorkspaceSnapshot,
  PaneStatus,
  SurfaceSnapshot,
} from "@/tauri/types";
import type { ProjectGroup } from "@/stores/app-store";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above imports, so spies they reference
// are created via `vi.hoisted`.
const {
  mockActivateWorkspace,
  mockDbGetUiState,
  mockSetActiveDraft,
  appStateHolder,
} = vi.hoisted(() => ({
  mockActivateWorkspace: vi.fn(),
  mockDbGetUiState: vi.fn(),
  mockSetActiveDraft: vi.fn(),
  appStateHolder: { current: null as unknown },
}));

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: (...args: unknown[]) => mockActivateWorkspace(...args),
  dbGetUiState: (...args: unknown[]) => mockDbGetUiState(...args),
}));

// The strip reads `appState.pane_statuses` to derive which workspaces are
// blocked; a hoisted holder lets each test inject a snapshot.
vi.mock("@/stores/app-store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/stores/app-store")>();
  const state = () => ({
    appState: appStateHolder.current,
    // Optimistic selection: the activation helper writes these before the
    // invoke (docs/plans/gui-responsiveness.md, Phase 1).
    pendingActiveWorkspaceId: null,
    pendingActivationAt: null,
    beginPendingActivation: vi.fn(),
    clearPendingActivation: vi.fn(),
  });
  return {
    // The real selectors run against the faked slice — the activation helper
    // reads `selectActiveWorkspaceId` to decide whether to open a trace.
    ...actual,
    useAppStore: Object.assign(
      vi.fn((selector: (s: Record<string, unknown>) => unknown) => selector(state())),
      { getState: state },
    ),
  };
});

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: {
    getState: () => ({ setActiveDraft: mockSetActiveDraft }),
  },
}));

// Late imports so the mocks above apply.
import { SidebarNeedsYouStrip } from "./sidebar-needs-you-strip";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import { useUIStore } from "@/stores/ui-store";

function surfaceWithPane(paneId: string): SurfaceSnapshot {
  return {
    surface_id: `sf-${paneId}`,
    title: "",
    root: { kind: "terminal", pane_id: paneId, session_id: "sess-1", title: "" },
    active_pane_id: paneId,
  };
}

function setPaneStatuses(statuses: Record<string, PaneStatus>) {
  appStateHolder.current = { pane_statuses: statuses };
}

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test Workspace",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
    git_branch: "feature/x",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/home/user/projects/myapp",
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

function group(
  projectName: string,
  projectPath: string,
  workspaces: WorkspaceSnapshot[],
): ProjectGroup {
  return { projectName, projectPath, workspaces };
}

beforeEach(() => {
  cleanup();
  appStateHolder.current = null;
  mockActivateWorkspace.mockReset();
  mockActivateWorkspace.mockResolvedValue(undefined);
  mockDbGetUiState.mockReset();
  mockDbGetUiState.mockResolvedValue(null);
  mockSetActiveDraft.mockReset();
  // scrollIntoView is unimplemented in jsdom — stub it so the jump handler
  // can call it without throwing.
  Element.prototype.scrollIntoView = vi.fn();
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
  });
  useUIStore.setState({ expandProjectRequest: null });
});

describe("SidebarNeedsYouStrip", () => {
  it("renders nothing when no workspace is waiting on the user", () => {
    setPaneStatuses({ "p-idle": "idle", "p-work": "working" });
    const groups = [
      group("MyApp", "/home/user/projects/myapp", [
        makeWorkspace({
          workspace_id: "ws-idle",
          surfaces: [surfaceWithPane("p-idle")],
        }),
        makeWorkspace({
          workspace_id: "ws-work",
          surfaces: [surfaceWithPane("p-work")],
        }),
      ]),
    ];
    const { container } = render(
      <SidebarNeedsYouStrip projectGroups={groups} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByText(/NEEDS YOU/)).not.toBeInTheDocument();
  });

  it("renders nothing when there is no app state yet", () => {
    appStateHolder.current = null;
    const groups = [
      group("MyApp", "/home/user/projects/myapp", [
        makeWorkspace({
          workspace_id: "ws-1",
          surfaces: [surfaceWithPane("p-1")],
        }),
      ]),
    ];
    const { container } = render(
      <SidebarNeedsYouStrip projectGroups={groups} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the header count and one entry per blocked workspace", () => {
    setPaneStatuses({
      "p-perm-a": "permission",
      "p-idle": "idle",
      "p-perm-b": "permission",
    });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-a",
          title: "Alpha work",
          surfaces: [surfaceWithPane("p-perm-a")],
        }),
        makeWorkspace({
          workspace_id: "ws-idle",
          surfaces: [surfaceWithPane("p-idle")],
        }),
      ]),
      group("Beta", "/home/user/projects/beta", [
        makeWorkspace({
          workspace_id: "ws-b",
          title: "Beta work",
          surfaces: [surfaceWithPane("p-perm-b")],
        }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    // Count reflects only the two permission workspaces.
    expect(screen.getByText(/NEEDS YOU · 2/)).toBeInTheDocument();
    // One jump-link per blocked workspace, each with the blocker fallback.
    const links = screen.getAllByRole("button", { name: /waiting for your input/i });
    expect(links).toHaveLength(2);
  });

  it("renders the blocker summary fallback and the age since the block", () => {
    useSidebarDensityStore.setState({
      statusSince: {
        "ws-perm": { status: "permission", at: Date.now() - 12 * 60_000 },
      },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-perm",
          title: "Alpha work",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    const { container } = render(
      <SidebarNeedsYouStrip projectGroups={groups} />,
    );
    expect(container.textContent).toContain("Waiting for your input");
    // Age derived from the density store's coarse status timestamp.
    expect(container.textContent).toContain("12m");
  });

  it("activates the blocked workspace and clears the chat draft on click", async () => {
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-target",
          title: "Alpha work",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    await userEvent.click(
      screen.getByRole("button", { name: /waiting for your input/i }),
    );

    expect(mockActivateWorkspace).toHaveBeenCalledTimes(1);
    expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-target");
    expect(mockSetActiveDraft).toHaveBeenCalledWith(null);
  });

  it("seeds a blocker's status timestamp so its age advances without a mounted row", () => {
    // A blocker inside a collapsed project group never mounts a row, so its
    // per-row `observeStatus` effect never runs. The strip must seed the
    // timestamp itself, otherwise the age reads "0s" forever.
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-collapsed",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    // No statusSince seeded up front (row never mounted).
    expect(
      useSidebarDensityStore.getState().statusSince["ws-collapsed"],
    ).toBeUndefined();

    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    const mark = useSidebarDensityStore.getState().statusSince["ws-collapsed"];
    expect(mark?.status).toBe("permission");
    expect(typeof mark?.at).toBe("number");
  });

  it("does not reset an already-observed blocker's timestamp (no-op on unchanged)", () => {
    // If a mounted row already stamped the block time, the strip's seeding
    // must not overwrite it — otherwise row + strip would fight and reset the
    // age on every render.
    const stampedAt = Date.now() - 20 * 60_000;
    useSidebarDensityStore.setState({
      statusSince: { "ws-perm": { status: "permission", at: stampedAt } },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-perm",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    expect(
      useSidebarDensityStore.getState().statusSince["ws-perm"]?.at,
    ).toBe(stampedAt);
  });

  it("requests expanding the blocked workspace's project group on click", async () => {
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-target",
          title: "Alpha work",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    await userEvent.click(
      screen.getByRole("button", { name: /waiting for your input/i }),
    );

    // The jump asks the (possibly collapsed) group to expand before scrolling
    // to the row, so the scrollIntoView target actually exists.
    expect(useUIStore.getState().expandProjectRequest).toBe(
      "/home/user/projects/alpha",
    );
  });

  it("orders entries oldest-blocked-first so the longest-waiting blocker survives the cap", () => {
    // Creation (tree) order is A→E, but they got blocked in the REVERSE
    // order: E has been waiting longest, A shortest. With five blockers and a
    // four-row cap, tree order would hide E — the one blocker the user most
    // needs to unblock — behind "+1 more below".
    const now = Date.now();
    useSidebarDensityStore.setState({
      statusSince: {
        "ws-a": { status: "permission", at: now - 10 * 60_000 },
        "ws-b": { status: "permission", at: now - 20 * 60_000 },
        "ws-c": { status: "permission", at: now - 30 * 60_000 },
        "ws-d": { status: "permission", at: now - 40 * 60_000 },
        "ws-e": { status: "permission", at: now - 50 * 60_000 },
      },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatuses({
      "p-a": "permission",
      "p-b": "permission",
      "p-c": "permission",
      "p-d": "permission",
      "p-e": "permission",
    });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({ workspace_id: "ws-a", title: "A", surfaces: [surfaceWithPane("p-a")] }),
        makeWorkspace({ workspace_id: "ws-b", title: "B", surfaces: [surfaceWithPane("p-b")] }),
        makeWorkspace({ workspace_id: "ws-c", title: "C", surfaces: [surfaceWithPane("p-c")] }),
        makeWorkspace({ workspace_id: "ws-d", title: "D", surfaces: [surfaceWithPane("p-d")] }),
        makeWorkspace({ workspace_id: "ws-e", title: "E", surfaces: [surfaceWithPane("p-e")] }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    // Header count reports all five blockers…
    expect(screen.getByText(/NEEDS YOU · 5/)).toBeInTheDocument();
    // …the visible rows run oldest-blocked-first…
    const links = screen.getAllByRole("button", {
      name: /waiting for your input/i,
    });
    expect(links.map((l) => l.getAttribute("aria-label"))).toEqual([
      "Jump to E — waiting for your input",
      "Jump to D — waiting for your input",
      "Jump to C — waiting for your input",
      "Jump to B — waiting for your input",
    ]);
    // …and the row the cap hides is the NEWEST blocker (A), never the oldest.
    expect(screen.getByText("+1 more below")).toBeInTheDocument();
  });

  it("ranks a blocker without a seeded timestamp as newest, ties in tree order", () => {
    // Only B has an observed block time; A and C are fresh blockers whose
    // timestamps haven't been stamped yet. B (the only one known to have
    // waited) must sort first; the unseeded pair keeps stable tree order.
    useSidebarDensityStore.setState({
      statusSince: {
        "ws-b": { status: "permission", at: Date.now() - 30 * 60_000 },
      },
      settledAt: {},
      lastSeenAt: {},
    });
    setPaneStatuses({
      "p-a": "permission",
      "p-b": "permission",
      "p-c": "permission",
    });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({ workspace_id: "ws-a", title: "A", surfaces: [surfaceWithPane("p-a")] }),
        makeWorkspace({ workspace_id: "ws-b", title: "B", surfaces: [surfaceWithPane("p-b")] }),
        makeWorkspace({ workspace_id: "ws-c", title: "C", surfaces: [surfaceWithPane("p-c")] }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    const links = screen.getAllByRole("button", {
      name: /waiting for your input/i,
    });
    expect(links.map((l) => l.getAttribute("aria-label"))).toEqual([
      "Jump to B — waiting for your input",
      "Jump to A — waiting for your input",
      "Jump to C — waiting for your input",
    ]);
  });

  it("sources the project chip color from the project.color UI-state key", () => {
    mockDbGetUiState.mockResolvedValue("#ef4444");
    setPaneStatuses({ "p-perm": "permission" });
    const groups = [
      group("Alpha", "/home/user/projects/alpha", [
        makeWorkspace({
          workspace_id: "ws-perm",
          surfaces: [surfaceWithPane("p-perm")],
        }),
      ]),
    ];
    render(<SidebarNeedsYouStrip projectGroups={groups} />);

    expect(mockDbGetUiState).toHaveBeenCalledWith(
      "project.color:/home/user/projects/alpha",
    );
    // The chip renders the project's leading letter.
    expect(screen.getByText("A")).toBeInTheDocument();
  });
});
