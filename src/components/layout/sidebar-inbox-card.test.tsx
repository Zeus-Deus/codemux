/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  ActivePaneStatus,
  AppStateSnapshot,
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
  getGithubIssue: vi.fn().mockResolvedValue(null),
  hostsList: vi.fn().mockResolvedValue([]),
  workspacePushToHost: vi.fn().mockResolvedValue({ ok: true, message: "" }),
  workspacePullBack: vi.fn().mockResolvedValue({ ok: true, message: "" }),
}));

vi.mock("@/stores/hosts-store", () => ({ useHosts: () => [] }));

function appStoreState() {
  return {
    appState: {
      workspaces: [],
      pane_statuses: {},
      active_workspace_id: "",
    } as unknown as AppStateSnapshot,
    homeDir: "/home/u",
    workspacePushPullInFlight: null,
    workspacePushPullStartedAt: null,
    setWorkspacePushPullInFlight: vi.fn(),
  };
}

vi.mock("@/stores/app-store", () => {
  const useAppStore = Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(appStoreState())),
    { getState: () => appStoreState() },
  );
  return { useAppStore, useHomeDir: () => "/home/u" };
});

// Late imports so the mocks above apply.
import { SidebarInboxCard } from "./sidebar-inbox-card";
import { activateWorkspace } from "@/tauri/commands";

const HOUR = 3_600_000;

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Ship it",
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
  } as WorkspaceSnapshot;
}

type CardProps = React.ComponentProps<typeof SidebarInboxCard>;

function renderCard(overrides: Partial<CardProps> = {}) {
  const props: CardProps = {
    workspace: makeWorkspace(),
    repo: { name: "myapp", path: "/home/u/projects/myapp" },
    isActive: false,
    status: null,
    showGitStats: true,
    now: 0,
    leaving: false,
    justUnsettled: false,
    onSettle: vi.fn(),
    onSnooze: vi.fn(),
    unread: false,
    woke: false,
    selected: false,
    onSelect: vi.fn(),
    onMarkUnread: vi.fn(),
    ...overrides,
  };
  const utils = render(
    <TooltipProvider>
      <SidebarInboxCard {...props} />
    </TooltipProvider>,
  );
  // The Settle/Snooze buttons carry the workspace title in their labels too,
  // so the card itself is addressed by its data attribute rather than by role.
  const card = utils.container.querySelector(
    "[data-inbox-card]",
  ) as HTMLElement;
  return { ...utils, props, card };
}

beforeEach(() => {
  vi.mocked(activateWorkspace).mockClear();
});
afterEach(cleanup);

describe("SidebarInboxCard — snooze affordance", () => {
  it("offers Snooze under the same guardrail as Settle", () => {
    for (const status of ["working", "permission"] as ActivePaneStatus[]) {
      renderCard({ status });
      expect(
        screen.queryByRole("button", { name: 'Snooze "Ship it"' }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: 'Settle "Ship it"' }),
      ).not.toBeInTheDocument();
      cleanup();
    }

    renderCard({ status: "review" });
    expect(
      screen.getByRole("button", { name: 'Snooze "Ship it"' }),
    ).toBeInTheDocument();
  });

  it("resolves the wake times when the menu opens, not when the card rendered", async () => {
    // Offsetting the real clock rather than freezing it keeps every timeout and
    // `waitFor` inside userEvent/testing-library moving forward normally.
    let offset = 0;
    const realNow = Date.now.bind(Date);
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockImplementation(() => realNow() + offset);
    try {
      const { props } = renderCard();
      const trigger = screen.getByRole("button", { name: 'Snooze "Ship it"' });

      await userEvent.click(trigger);
      await userEvent.click(
        await screen.findByRole("menuitem", { name: /In 1 hour/ }),
      );
      const first = vi.mocked(props.onSnooze).mock.calls[0][1];

      // Time passes with the card still mounted and never re-rendered by a
      // clock tick. A preset resolved at render time would still be quoting
      // the old instant here.
      offset = 3 * HOUR;
      await userEvent.click(trigger);
      await userEvent.click(
        await screen.findByRole("menuitem", { name: /In 1 hour/ }),
      );
      const second = vi.mocked(props.onSnooze).mock.calls[1][1];

      expect(second - first).toBeGreaterThanOrEqual(3 * HOUR);
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("names the concrete wake time beside each relative label", async () => {
    renderCard();
    await userEvent.click(screen.getByRole("button", { name: 'Snooze "Ship it"' }));

    const nextWeek = await screen.findByText("Next week");
    // "Next week" alone never says which day or hour; the absolute half is what
    // stops a deferral being a guess.
    expect(nextWeek.closest('[role="menuitem"]')?.textContent).toMatch(
      /Next week.+0?9[:.]00/,
    );
  });

  it("keeps the hover cluster pinned open while the snooze menu is open", async () => {
    renderCard();
    const trigger = screen.getByRole("button", { name: 'Snooze "Ship it"' });
    // At rest the cluster is CSS-hidden and only revealed by hover/focus.
    expect(trigger.className).toContain("hidden");

    await userEvent.click(trigger);

    // With the menu open the reveal is state-driven, so the pointer leaving
    // the card can no longer collapse the trigger out from under it.
    expect(trigger.className).not.toContain("hidden");
    expect(trigger.className).toContain("inline-flex");
  });

  it("snoozes to the chosen preset's instant without activating the workspace", async () => {
    const { props } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: 'Snooze "Ship it"' }));
    await userEvent.click(await screen.findByText("Tomorrow"));

    const [id, until] = vi.mocked(props.onSnooze).mock.calls[0];
    expect(id).toBe("ws-1");
    // Tomorrow morning, not an arithmetic day from now.
    expect(new Date(until).getHours()).toBe(9);
    expect(activateWorkspace).not.toHaveBeenCalled();
  });
});

describe("SidebarInboxCard — unread / woke", () => {
  it("marks an unread card without borrowing the done-review green", () => {
    const { container } = renderCard({ unread: true });
    const dot = screen.getByLabelText('Unread — "Ship it"');
    expect(dot.className).toContain("bg-accent-ember");
    expect(container.querySelector(".font-bold")).toBeInTheDocument();
  });

  it("shows the woke pill alongside an unread dot", () => {
    renderCard({ unread: true, woke: true });
    expect(screen.getByLabelText('"Ship it" woke from snooze')).toBeInTheDocument();
    expect(screen.getByLabelText('Unread — "Ship it"')).toBeInTheDocument();
  });

  it("offers Mark unread only while the card is read", async () => {
    const { props, card } = renderCard();
    await userEvent.pointer({ keys: "[MouseRight]", target: card });
    await userEvent.click(await screen.findByText("Mark unread"));
    expect(props.onMarkUnread).toHaveBeenCalledWith("ws-1");

    cleanup();
    const alreadyUnread = renderCard({ unread: true });
    await userEvent.pointer({
      keys: "[MouseRight]",
      target: alreadyUnread.card,
    });
    expect(await screen.findByText("Rename workspace")).toBeInTheDocument();
    expect(screen.queryByText("Mark unread")).not.toBeInTheDocument();
  });
});

describe("SidebarInboxCard — multi-select", () => {
  it("activates on a plain click and collapses the selection to this card", async () => {
    const { props, card } = renderCard();
    await userEvent.click(card);
    expect(props.onSelect).toHaveBeenCalledWith("ws-1", "single");
    expect(activateWorkspace).toHaveBeenCalledWith("ws-1");
  });

  it("selects without activating on a modified click", async () => {
    const { props, card } = renderCard();

    // fireEvent rather than userEvent: the modifier has to ride on the click
    // itself, and userEvent's held-key state does not survive between its
    // one-shot top-level calls.
    fireEvent.click(card, { ctrlKey: true });
    expect(props.onSelect).toHaveBeenLastCalledWith("ws-1", "toggle");

    fireEvent.click(card, { metaKey: true });
    expect(props.onSelect).toHaveBeenLastCalledWith("ws-1", "toggle");

    fireEvent.click(card, { shiftKey: true });
    expect(props.onSelect).toHaveBeenLastCalledWith("ws-1", "range");

    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("renders selection distinctly from the active treatment", () => {
    const { container } = renderCard({ selected: true, isActive: false });
    const card = container.querySelector("[data-inbox-card]");
    expect(card).toHaveAttribute("data-selected", "true");
    expect(card?.className).toContain("ring-accent-ember/55");
  });
});
