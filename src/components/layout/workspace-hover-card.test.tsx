/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, act } from "@testing-library/react";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import type {
  AppStateSnapshot,
  PortInfoSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";
import type { HostView } from "@/tauri/commands";

let detectedPorts: PortInfoSnapshot[] = [];
let hosts: HostView[] = [];
let homeDir: string | null = "/home/u";

vi.mock("@/stores/app-store", () => {
  const state = () =>
    ({
      appState: { detected_ports: detectedPorts } as unknown as AppStateSnapshot,
    }) as unknown;
  return {
    useAppStore: Object.assign(
      vi.fn((selector: (s: unknown) => unknown) => selector(state())),
      { getState: state },
    ),
    useHomeDir: () => homeDir,
  };
});

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => hosts,
}));

vi.mock("./use-project-appearance", () => ({
  useProjectAppearance: () => ({
    customColor: null,
    imageUrl: null,
    imageVersion: 0,
  }),
}));

import { WorkspaceHoverCardBody } from "./workspace-hover-card";

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "my-workspace",
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
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

function makePort(overrides: Partial<PortInfoSnapshot> = {}): PortInfoSnapshot {
  return {
    port: 3000,
    pid: 1,
    process_name: "node",
    workspace_id: "ws-1",
    label: null,
    source: null,
    ...overrides,
  };
}

function renderBody(
  workspace: WorkspaceSnapshot,
  status: Parameters<typeof WorkspaceHoverCardBody>[0]["status"] = null,
) {
  return render(
    <WorkspaceHoverCardBody
      workspace={workspace}
      repo={{ name: "myapp", path: "/home/u/projects/myapp" }}
      status={status}
    />,
  );
}

/** The value rendered next to a given label row. */
function valueFor(label: string): string {
  const labelEl = screen.getByText(label);
  return labelEl.nextElementSibling?.textContent ?? "";
}

beforeEach(() => {
  detectedPorts = [];
  hosts = [];
  homeDir = "/home/u";
});

afterEach(cleanup);

describe("WorkspaceHoverCardBody — header", () => {
  it("shows the repo name and the FULL title (the row truncates it)", () => {
    renderBody(
      makeWorkspace({
        title: "a-very-long-workspace-name-the-sidebar-row-cannot-show",
      }),
    );
    expect(screen.getByText("myapp")).toBeInTheDocument();
    expect(
      screen.getByText("a-very-long-workspace-name-the-sidebar-row-cannot-show"),
    ).toBeInTheDocument();
  });

  it("renders the linked issue number and title under the workspace title", () => {
    renderBody(
      makeWorkspace({
        linked_issue: {
          number: 42,
          title: "Fix the flaky sidebar test",
          state: "Open",
          labels: [],
        },
      }),
    );
    expect(screen.getByText("#42")).toBeInTheDocument();
    expect(screen.getByText(/Fix the flaky sidebar test/)).toBeInTheDocument();
  });

  it("labels each agent status with its own tone, and idle when there is none", () => {
    const { unmount } = renderBody(makeWorkspace(), "working");
    expect(screen.getByText("Working")).toHaveClass("text-status-working");
    unmount();

    const needs = renderBody(makeWorkspace(), "permission");
    expect(screen.getByText("Needs you")).toHaveClass("text-status-attention");
    needs.unmount();

    const review = renderBody(makeWorkspace(), "review");
    expect(screen.getByText("Done · review")).toHaveClass("text-status-open");
    review.unmount();

    renderBody(makeWorkspace(), null);
    expect(screen.getByText("Idle")).toBeInTheDocument();
  });

  it("ticks the elapsed label on the coarse clock while the card stays open", () => {
    // Regression: elapsed was stamped once per mount, so "Working 1m" froze
    // for as long as the pointer rested on the card.
    vi.useFakeTimers();
    try {
      useSidebarDensityStore.setState({
        statusSince: {
          "ws-1": { status: "working", at: Date.now() - 60_000 },
        },
      });
      renderBody(makeWorkspace(), "working");
      expect(screen.getByText("1m")).toBeInTheDocument();

      act(() => {
        vi.advanceTimersByTime(4 * 60_000);
      });
      expect(screen.getByText("5m")).toBeInTheDocument();
    } finally {
      useSidebarDensityStore.setState({ statusSince: {} });
      vi.useRealTimers();
    }
  });
});

describe("WorkspaceHoverCardBody — git rows", () => {
  it("surfaces behind + changed-file counts the sidebar row never shows", () => {
    renderBody(
      makeWorkspace({
        git_branch: "feature/x",
        git_ahead: 2,
        git_behind: 3,
        git_additions: 10,
        git_deletions: 4,
        git_changed_files: 6,
      }),
    );
    expect(valueFor("Branch")).toBe("feature/x");
    expect(valueFor("Uncommitted")).toBe("+10 −4");
    expect(valueFor("Changed files")).toBe("6");
    expect(valueFor("Ahead")).toBe("↑2");
    expect(valueFor("Behind")).toBe("↓3");
  });

  it("states a clean working tree outright rather than showing nothing", () => {
    renderBody(makeWorkspace({ git_branch: "main" }));
    expect(valueFor("Working tree")).toBe("clean");
    expect(screen.queryByText("Uncommitted")).not.toBeInTheDocument();
    expect(screen.queryByText("Changed files")).not.toBeInTheDocument();
  });

  it("omits ahead/behind rows when there is nothing to report", () => {
    renderBody(makeWorkspace({ git_ahead: 0, git_behind: 0 }));
    expect(screen.queryByText("Ahead")).not.toBeInTheDocument();
    expect(screen.queryByText("Behind")).not.toBeInTheDocument();
  });

  it("drops every git row for a non-git workspace", () => {
    renderBody(
      makeWorkspace({
        is_git: false,
        git_branch: "main",
        git_additions: 5,
        git_changed_files: 2,
      }),
    );
    for (const label of [
      "Branch",
      "Uncommitted",
      "Changed files",
      "Working tree",
    ]) {
      expect(screen.queryByText(label)).not.toBeInTheDocument();
    }
  });
});

describe("WorkspaceHoverCardBody — PR, issue, ports", () => {
  it("shows PR number and state with the shared PR tone", () => {
    renderBody(makeWorkspace({ pr_number: 140, pr_state: "merged" }));
    expect(valueFor("Pull request")).toBe("#140 · merged");
    expect(screen.getByText("#140 · merged")).toHaveClass("text-accent-violet");
  });

  it("names the PR's own branch only when it is not the checked-out one", () => {
    // A side-branch association: the badge is real, but the Branch row says
    // something else, and this row is what answers "why?". It also explains
    // why merging that PR will not settle the card.
    const { unmount } = renderBody(
      makeWorkspace({
        git_branch: "fix-ui-borders",
        pr_number: 250,
        pr_state: "open",
        pr_head_branch: "appimage-child-env-hygiene",
      }),
    );
    expect(valueFor("PR branch")).toBe("appimage-child-env-hygiene");
    unmount();

    // Matching association — the row would only repeat the Branch row above.
    renderBody(
      makeWorkspace({
        git_branch: "fix-ui-borders",
        pr_number: 251,
        pr_state: "open",
        pr_head_branch: "fix-ui-borders",
      }),
    );
    expect(screen.queryByText("PR branch")).not.toBeInTheDocument();
  });

  it("omits the PR branch row for a pre-field snapshot", () => {
    // No head branch recorded means "association predates the field", not
    // "side branch" — inventing a mismatch there would be a lie.
    renderBody(
      makeWorkspace({ git_branch: "main", pr_number: 9, pr_state: "open" }),
    );
    expect(screen.queryByText("PR branch")).not.toBeInTheDocument();
  });

  it("tones an open issue as success and a closed one as muted", () => {
    const { unmount } = renderBody(
      makeWorkspace({
        linked_issue: { number: 7, title: "t", state: "Open", labels: [] },
      }),
    );
    expect(screen.getByText("#7 · Open")).toHaveClass("text-success");
    unmount();

    renderBody(
      makeWorkspace({
        linked_issue: { number: 7, title: "t", state: "Closed", labels: [] },
      }),
    );
    expect(screen.getByText("#7 · Closed")).toHaveClass("text-muted-foreground");
  });

  it("lists this workspace's ports only, singular label for one", () => {
    detectedPorts = [
      makePort({ port: 3000 }),
      makePort({ port: 5173, workspace_id: "other-ws" }),
    ];
    renderBody(makeWorkspace());
    expect(valueFor("Port")).toBe(":3000");
  });

  it("caps the port list at three and counts the overflow", () => {
    detectedPorts = [3000, 3001, 3002, 3003, 3004].map((port) =>
      makePort({ port }),
    );
    renderBody(makeWorkspace());
    expect(valueFor("Ports")).toBe(":3000 :3001 :3002 +2");
  });

  it("omits the ports row when none are detected", () => {
    renderBody(makeWorkspace());
    expect(screen.queryByText("Port")).not.toBeInTheDocument();
    expect(screen.queryByText("Ports")).not.toBeInTheDocument();
  });
});

describe("WorkspaceHoverCardBody — location, mute, path", () => {
  it("reads 'This device' for a local workspace", () => {
    renderBody(makeWorkspace());
    expect(valueFor("Location")).toBe("This device");
  });

  it("names the host for a remote workspace", () => {
    hosts = [{ id: 3, name: "beelink" } as HostView];
    renderBody(makeWorkspace({ host_id: 3 }));
    expect(valueFor("Location")).toBe("beelink");
    expect(screen.getByText("beelink")).toHaveClass("text-status-remote");
  });

  it("still reads as remote when the hosts list has not resolved the name yet", () => {
    // hosts load asynchronously; an unresolved lookup must not claim "local".
    hosts = [];
    renderBody(makeWorkspace({ host_id: 9 }));
    expect(valueFor("Location")).toBe("Another device");
    expect(screen.getByText("Another device")).toHaveClass("text-status-remote");
  });

  it("marks an attach-in-place workspace as running on the host", () => {
    hosts = [{ id: 3, name: "beelink" } as HostView];
    renderBody(makeWorkspace({ host_id: 3, attach_only: true }));
    expect(valueFor("Location")).toBe("beelink · in place");
  });

  it("shows the muted row only when notifications are muted", () => {
    const { unmount } = renderBody(makeWorkspace({ notifications_muted: true }));
    expect(valueFor("Notifications")).toBe("muted");
    unmount();

    renderBody(makeWorkspace({ notifications_muted: false }));
    expect(screen.queryByText("Notifications")).not.toBeInTheDocument();
  });

  it("prefers the worktree path and collapses $HOME to ~", () => {
    renderBody(
      makeWorkspace({
        cwd: "/home/u/projects/myapp",
        worktree_path: "/home/u/.codemux/worktrees/myapp/feat-x",
      }),
    );
    expect(
      screen.getByText("~/.codemux/worktrees/myapp/feat-x"),
    ).toBeInTheDocument();
  });

  it("keeps a sibling-prefix path absolute (home /home/u vs /home/u2)", () => {
    // Regression: a bare startsWith(homeDir) shortened /home/u2/project to
    // "~2/project". Only a real path-separator boundary counts as home.
    renderBody(makeWorkspace({ cwd: "/home/u2/project" }));
    expect(screen.getByText("/home/u2/project")).toBeInTheDocument();
    expect(screen.queryByText("~2/project")).not.toBeInTheDocument();
  });

  it("collapses a path that IS the home dir to a bare ~", () => {
    renderBody(makeWorkspace({ cwd: "/home/u" }));
    expect(screen.getByText("~")).toBeInTheDocument();
  });

  it("shortens under a home dir reported with a trailing separator", () => {
    homeDir = "/home/u/";
    renderBody(makeWorkspace({ cwd: "/home/u/projects/myapp" }));
    expect(screen.getByText("~/projects/myapp")).toBeInTheDocument();
  });

  it("falls back to the remote cwd for an attach-in-place workspace", () => {
    hosts = [{ id: 1, name: "box" } as HostView];
    renderBody(
      makeWorkspace({
        host_id: 1,
        attach_only: true,
        worktree_path: null,
        remote_cwd: "/srv/work/myapp",
      }),
    );
    // Not under $HOME, so it stays absolute rather than gaining a bogus "~".
    expect(screen.getByText("/srv/work/myapp")).toBeInTheDocument();
  });
});
