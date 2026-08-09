/// <reference types="@testing-library/jest-dom/vitest" />
import { StrictMode } from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  cleanup,
  act,
  fireEvent,
} from "@testing-library/react";
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

import {
  WorkspaceHoverCard,
  WorkspaceHoverCardBody,
} from "./workspace-hover-card";
import { __resetHoverCardGroupForTests } from "@/lib/hover-card-group";

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

  it("shows the matching running-process indicator as the final detail row", () => {
    detectedPorts = [
      makePort({ port: 3000, pid: 10 }),
      makePort({ port: 3001, pid: 10 }),
      makePort({ port: 5173, pid: 20 }),
    ];
    const { container } = renderBody(makeWorkspace());

    expect(valueFor("Processes")).toBe("2 running");
    expect(screen.getByLabelText("2 running processes")).toBeInTheDocument();
    expect(container.querySelector(".lucide-terminal")).toBeInTheDocument();

    const detailRows = container.querySelectorAll(".flex.flex-col > div");
    expect(detailRows.item(detailRows.length - 1)).toHaveTextContent(
      "Processes2 running",
    );
  });

  it("omits the ports row when none are detected", () => {
    renderBody(makeWorkspace());
    expect(screen.queryByText("Port")).not.toBeInTheDocument();
    expect(screen.queryByText("Ports")).not.toBeInTheDocument();
    expect(screen.queryByText("Process")).not.toBeInTheDocument();
    expect(screen.queryByText("Processes")).not.toBeInTheDocument();
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

describe("WorkspaceHoverCard — hover timing", () => {
  const OPEN_DELAY_MS = 150;
  const CLOSE_DELAY_MS = 100;
  const GROUP_TIMEOUT_MS = 400;

  beforeEach(() => {
    vi.useFakeTimers();
    __resetHoverCardGroupForTests();
  });

  afterEach(() => {
    __resetHoverCardGroupForTests();
    vi.useRealTimers();
  });

  /** Two sidebar rows, as the inbox renders them. `strict` double-invokes
   *  render and state updaters, which is how the app itself runs. */
  function renderRows({ strict = false }: { strict?: boolean } = {}) {
    const rows = (
      <>
        {["alpha", "beta"].map((title) => (
          <WorkspaceHoverCard
            key={title}
            workspace={makeWorkspace({ workspace_id: title, title })}
            repo={{ name: "myapp", path: "/home/u/projects/myapp" }}
            status={null}
          >
            <button type="button">{`row-${title}`}</button>
          </WorkspaceHoverCard>
        ))}
      </>
    );
    render(strict ? <StrictMode>{rows}</StrictMode> : rows);
    return {
      alpha: screen.getByText("row-alpha"),
      beta: screen.getByText("row-beta"),
    };
  }

  // React synthesises onPointerEnter/Leave from pointerover/pointerout, so
  // firing the enter/leave events directly would never reach Radix.
  function pointerEnter(el: HTMLElement) {
    act(() => {
      fireEvent.pointerOver(el, { pointerType: "mouse", relatedTarget: null });
    });
  }
  function pointerLeave(el: HTMLElement) {
    act(() => {
      fireEvent.pointerOut(el, {
        pointerType: "mouse",
        relatedTarget: document.body,
      });
    });
  }
  function advance(ms: number) {
    act(() => {
      vi.advanceTimersByTime(ms);
    });
  }

  /** The open card's content element, or null. */
  function card(): HTMLElement | null {
    return document.querySelector("[data-slot='hover-card-content']");
  }

  it("holds the first card back by the open delay, so a sweep past a row never flashes it", () => {
    const { alpha } = renderRows();
    pointerEnter(alpha);

    advance(OPEN_DELAY_MS - 1);
    expect(card()).toBeNull();

    advance(1);
    expect(card()).not.toBeNull();
    expect(screen.getByText("alpha")).toBeInTheDocument();
  });

  it("opens the NEXT row's card with no delay at all once a card is already up", () => {
    const { alpha, beta } = renderRows();
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);
    expect(screen.getByText("alpha")).toBeInTheDocument();

    pointerLeave(alpha);
    pointerEnter(beta);
    // Not one tick of delay — the timer still has to fire, but at 0ms.
    advance(0);
    expect(screen.getByText("beta")).toBeInTheDocument();
  });

  it("retires the previous card the instant the next one opens, instead of holding it for its close delay", () => {
    const { alpha, beta } = renderRows();
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);

    pointerLeave(alpha);
    pointerEnter(beta);
    advance(0);

    // `alpha` would otherwise linger for its close delay, stacked over `beta`.
    // jsdom runs no animations and unmounts synchronously, so the retiring card
    // is simply gone here; a browser plays its exit fade over `beta`, i.e. a
    // brief crossfade rather than a hard cut.
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(document.querySelectorAll("[data-slot='hover-card-content']")).toHaveLength(1);
  });

  it("keeps the close delay for the pointer travelling into the card itself", () => {
    const { alpha } = renderRows();
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);

    pointerLeave(alpha);
    advance(CLOSE_DELAY_MS - 1);
    // Still up: this is the window in which the pointer crosses the offset gap
    // to select the path or the branch.
    expect(screen.getByText("alpha")).toBeInTheDocument();

    advance(1);
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
  });

  it("marks only the cards that skipped the delay as instant, so a deliberate hover still animates in", () => {
    const { alpha, beta } = renderRows();
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);
    expect(card()).not.toHaveAttribute("data-instant");

    pointerLeave(alpha);
    pointerEnter(beta);
    advance(0);
    expect(card()).toHaveAttribute("data-instant");
  });

  it("marks instant the same way under StrictMode, where state updaters run twice", () => {
    // The phase is read once, before `setCardState`, precisely so the answer
    // cannot depend on how many times React chooses to run the updater — by
    // the second run this card has already joined the phase it is asking about.
    const { alpha, beta } = renderRows({ strict: true });
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);
    expect(card()).not.toHaveAttribute("data-instant");

    pointerLeave(alpha);
    pointerEnter(beta);
    advance(0);
    expect(card()).toHaveAttribute("data-instant");
  });

  it("goes back to the full delay once the group phase has lapsed", () => {
    const { alpha, beta } = renderRows();
    pointerEnter(alpha);
    advance(OPEN_DELAY_MS);
    pointerLeave(alpha);
    // Two steps: the grace window is only armed once the close has committed
    // and the card's effect cleanup has run, which act() defers to its flush.
    advance(CLOSE_DELAY_MS);
    advance(GROUP_TIMEOUT_MS);

    pointerEnter(beta);
    advance(OPEN_DELAY_MS - 1);
    expect(card()).toBeNull();
    advance(1);
    expect(screen.getByText("beta")).toBeInTheDocument();
  });
});

// ── Provider-aware detail rows ──

describe("hosting provider", () => {
  it("keeps GitHub's label and `#` sigil", () => {
    renderBody(
      makeWorkspace({
        pr_number: 42,
        pr_state: "OPEN",
        pr_url: "https://github.com/acme/app/pull/42",
        provider_kind: "github",
      }),
    );
    expect(valueFor("Pull request")).toContain("#42");
  });

  it("uses GitLab's label and `!` sigil", () => {
    renderBody(
      makeWorkspace({
        pr_number: 42,
        pr_state: "OPEN",
        pr_url: "https://gitlab.acme.com/acme/app/-/merge_requests/42",
        provider_kind: "gitlab",
      }),
    );
    expect(valueFor("Merge request")).toContain("!42");
    expect(screen.queryByText("Pull request")).not.toBeInTheDocument();
  });

  it("names a self-hosted instance on the Hosting row", () => {
    renderBody(
      makeWorkspace({
        pr_number: 42,
        pr_state: "OPEN",
        pr_url: "https://gitlab.acme.com/acme/app/-/merge_requests/42",
        provider_kind: "gitlab",
      }),
    );
    expect(valueFor("Hosting")).toBe("GitLab · gitlab.acme.com");
  });

  it("omits the product's own domain from the Hosting row", () => {
    renderBody(
      makeWorkspace({
        pr_number: 1,
        pr_state: "OPEN",
        pr_url: "https://gitlab.com/acme/app/-/merge_requests/1",
        provider_kind: "gitlab",
      }),
    );
    expect(valueFor("Hosting")).toBe("GitLab");
  });

  it("shows no Hosting row when detection never classified the checkout", () => {
    // Absent `provider_kind` means "not detected", not "GitHub". Copy
    // falls back to GitHub wording, but asserting a detection result
    // that does not exist would be a different claim entirely.
    renderBody(
      makeWorkspace({
        pr_number: 7,
        pr_state: "OPEN",
        pr_url: "https://github.com/acme/app/pull/7",
      }),
    );
    expect(screen.queryByText("Hosting")).not.toBeInTheDocument();
    expect(valueFor("Pull request")).toContain("#7");
  });

  it("labels a side-branch row with the product's abbreviation", () => {
    renderBody(
      makeWorkspace({
        git_branch: "main",
        pr_number: 9,
        pr_state: "OPEN",
        pr_head_branch: "feature/x",
        pr_url: "https://gitlab.com/acme/app/-/merge_requests/9",
        provider_kind: "gitlab",
      }),
    );
    expect(screen.getByText("MR branch")).toBeInTheDocument();
  });
});
