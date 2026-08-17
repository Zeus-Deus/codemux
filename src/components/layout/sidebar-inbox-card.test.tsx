/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render,
  screen,
  cleanup,
  fireEvent,
  within,
} from "@testing-library/react";
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
    // Optimistic selection: the activation helper writes these before invoke.
    pendingActiveWorkspaceId: null,
    pendingActivationAt: null,
    beginPendingActivation: vi.fn(),
    clearPendingActivation: vi.fn(),
  };
}

vi.mock("@/stores/app-store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/stores/app-store")>();
  const useAppStore = Object.assign(
    vi.fn((selector: (s: unknown) => unknown) => selector(appStoreState())),
    { getState: () => appStoreState() },
  );
  // The real selectors run against the faked slice — the activation helper
  // reads `selectActiveWorkspaceId` to decide whether to open a trace.
  return { ...actual, useAppStore, useHomeDir: () => "/home/u" };
});

// Late imports so the mocks above apply.
import {
  SidebarInboxCard,
  META_CLUSTER_MIN_WIDTH,
  metaClusterWidth,
} from "./sidebar-inbox-card";
import { activateWorkspace } from "@/tauri/commands";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";

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

function metaLine(container: HTMLElement) {
  return container.querySelector("[data-meta-line]") as HTMLElement;
}

beforeEach(() => {
  vi.mocked(activateWorkspace).mockClear();
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
  });
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
    // Pinned to a Wednesday: on Sundays the "Next week" preset is
    // deliberately withheld (it would duplicate "Tomorrow"), and this test
    // is about the label, not that rule.
    const nowSpy = vi
      .spyOn(Date, "now")
      .mockReturnValue(new Date(2026, 5, 10, 12, 0, 0).getTime());
    try {
      renderCard();
      await userEvent.click(
        screen.getByRole("button", { name: 'Snooze "Ship it"' }),
      );

      const nextWeek = await screen.findByText("Next week");
      // "Next week" alone never says which day or hour; the absolute half is
      // what stops a deferral being a guess.
      expect(nextWeek.closest('[role="menuitem"]')?.textContent).toMatch(
        /Next week.+0?9[:.]00/,
      );
    } finally {
      nowSpy.mockRestore();
    }
  });

  it("keeps the hover cluster pinned open while the snooze menu is open", async () => {
    const { card } = renderCard();
    const trigger = screen.getByRole("button", { name: 'Snooze "Ship it"' });
    // The reveal rides the cluster the buttons share, not each button, so the
    // three actions can never half-appear.
    const cluster = trigger.parentElement!;
    // At rest the cluster is CSS-hidden and only revealed by hover/focus.
    expect(cluster.className).toContain("hidden");

    await userEvent.click(trigger);

    // With the menu open the reveal is state-driven, so the pointer leaving
    // the card can no longer collapse the trigger out from under it.
    expect(cluster.className).not.toContain("hidden");
    expect(cluster.className).toContain("flex");
    // The menu portal owns pointer/focus now, but the card must stay visually
    // restored for the duration of that interaction.
    expect(within(card).getByText("M").className).not.toContain("grayscale");
    expect(within(card).getByText("Ship it").className).toContain(
      "text-foreground",
    );
  });

  it("snoozes to the chosen preset's instant without activating the workspace", async () => {
    const { props } = renderCard();
    await userEvent.click(
      screen.getByRole("button", { name: 'Snooze "Ship it"' }),
    );
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
    expect(
      screen.getByLabelText('"Ship it" woke from snooze'),
    ).toBeInTheDocument();
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

describe("SidebarInboxCard — background recede", () => {
  // Prominence is reserved for rows that want a human: the one you are in,
  // needs-you, done-review, unread, woke, and anything ticked for a bulk
  // action. A quietly-working agent and an idle row you have already read sit
  // back instead, and hover/focus brings them straight back to full.
  //
  // The recede is per element, never one opacity on the card root — that is
  // what lets the state readout stay exempt, so a background card still says
  // out loud what its agent is doing.
  it("recedes a quietly-working card without dimming the whole node", () => {
    const { card } = renderCard({ status: "working" });
    expect(card.className).not.toContain("opacity-70");
    expect(screen.getByText("M").className).toContain("grayscale");
    expect(screen.getByText("Ship it").className).toContain(
      "text-muted-foreground/55",
    );
  });

  it("keeps the working readout at full strength on a receded card", () => {
    renderCard({ status: "working" });

    const workingState = screen.getByText("Working").parentElement;
    // Amber and semibold at rest: no hover required to tell a live agent
    // from an idle row.
    expect(workingState?.className).toContain("text-status-working");
    expect(workingState?.className).toContain("font-semibold");
    expect(workingState?.className).not.toContain("text-muted-foreground");
  });

  it("neutralizes a background card's colors until hover or focus", () => {
    const { card } = renderCard({
      status: "working",
      workspace: makeWorkspace({
        git_additions: 38,
        git_deletions: 12,
        pr_number: 87,
        pr_state: "open",
        pr_url: "https://example.test/pr/87",
      }),
    });

    const avatar = screen.getByText("M");
    expect(avatar.className).toContain("grayscale");
    expect(avatar.className).toContain("opacity-40");
    expect(avatar.className).toContain("group-hover/card:grayscale-0");

    const title = screen.getByText("Ship it");
    expect(title.className).toContain("text-muted-foreground/55");
    expect(title.className).toContain("group-hover/card:text-foreground");

    // The meta line mutes as a block, so branch and ↑ahead inherit it.
    expect(metaLine(card).className).toContain("text-muted-foreground/40");
    expect(metaLine(card).className).toContain(
      "group-hover/card:text-muted-foreground/60",
    );

    const additions = screen.getByText("+38");
    const deletions = screen.getByText("−12");
    expect(additions.className).toContain("text-muted-foreground/50");
    expect(additions.className).toContain(
      "group-hover/card:text-status-open/80",
    );
    expect(deletions.className).toContain("text-muted-foreground/50");
    expect(deletions.className).toContain(
      "group-focus-within/card:text-status-attention/80",
    );

    const pr = screen.getByRole("button", { name: "Pull request #87 — open" });
    expect(pr.className).toContain("text-muted-foreground/45");
    expect(pr.className).toContain("group-hover/card:text-status-open");
    expect(pr.className).not.toMatch(/(^|\s)text-status-open\b/);
    expect(pr.querySelector("svg")?.getAttribute("class")).toContain(
      "text-current",
    );
    // None of that dimming rides a blanket opacity on the card node.
    expect(card.className).not.toContain("opacity-70");
  });

  // Idle must not light up: the exemption exists so a *live* agent reads as
  // live, and a row with nothing running has no state to shout about — so its
  // timestamp dims with the rest of the card instead of outshining the title.
  it("recedes an idle background card along with its readout", () => {
    useSidebarDensityStore.setState({ settledAt: { "ws-1": 0 } });
    const { card } = renderCard({ status: null, now: 26 * 60_000 });

    expect(card.className).not.toContain("opacity-70");
    expect(screen.getByText("Ship it").className).toContain(
      "text-muted-foreground/55",
    );

    const idleState = screen.getByText("26m");
    expect(idleState.className).toContain("text-muted-foreground/50");
    expect(idleState.className).toContain(
      "group-hover/card:text-muted-foreground/70",
    );
    expect(idleState.className).toContain("font-medium");
    expect(idleState.className).not.toContain("text-status-");
  });

  // The chip paints its own colours, so the recede has to ride a wrapper
  // opacity — otherwise it is the brightest mark on a dim card.
  it("dims the linked-issue chip on a receded working card", () => {
    renderCard({
      status: "working",
      workspace: makeWorkspace({
        linked_issue: { number: 42, title: "Fix it", state: "Open", labels: [] },
      }),
    });

    const chipWrapper = screen.getByText("#42").closest("button")?.parentElement;
    expect(chipWrapper?.className).toContain("opacity-70");
    expect(chipWrapper?.className).toContain("group-hover/card:opacity-100");
    expect(chipWrapper?.className).toContain(
      "group-focus-within/card:opacity-100",
    );
  });

  // The pin marker is eyebrow furniture, not a live readout, so it recedes
  // with the repo name it sits beside.
  it("dims the resting pin marker on a receded card", () => {
    const { container } = renderCard({ pinned: true, status: "working" });
    // An SVG node, so `className` is an SVGAnimatedString — read the attribute.
    const marker = container.querySelector('[aria-label="Pinned workspace"]')!;
    const classes = marker.getAttribute("class") ?? "";
    expect(classes).toContain("text-muted-foreground/55");
    expect(classes).toContain("group-hover/card:text-muted-foreground/75");
    expect(classes).not.toMatch(/(^|\s)text-muted-foreground\/75\b/);
  });

  // A watch loop is background presence by definition, so its card recedes
  // with the quietly-working rows — but monitoring is a live state, so its
  // readout keeps its own colour just as Working does.
  it("recedes a monitoring card while its readout keeps its tone", () => {
    const { card } = renderCard({ status: "monitoring" as ActivePaneStatus });
    expect(card.className).not.toContain("opacity-70");
    expect(screen.getByText("Ship it").className).toContain(
      "text-muted-foreground/55",
    );

    const label = screen.getByText("Monitoring");
    expect(label.className).toContain("text-status-monitoring");
    expect(label.className).toContain("font-semibold");
  });

  it("keeps every card that wants a human at full brightness", () => {
    const cases: Array<[string, Partial<CardProps>]> = [
      ["the workspace you are in", { isActive: true, status: "working" }],
      [
        "a card ticked for a bulk action",
        { selected: true, status: "working" },
      ],
      ["unread agent output", { unread: true, status: "working" }],
      ["a card just back from a snooze", { woke: true, status: null }],
      ["an agent blocked on you", { status: "permission" as ActivePaneStatus }],
      [
        "work finished and waiting on a review",
        { status: "review" as ActivePaneStatus },
      ],
    ];

    for (const [label, overrides] of cases) {
      const { card } = renderCard(overrides);
      // Nothing on these cards carries the receded treatment: the title is
      // full-strength ink and the repo avatar keeps its colour.
      expect(screen.getByText("Ship it").className, label).toContain(
        "text-foreground",
      );
      expect(screen.getByText("Ship it").className, label).not.toContain(
        "text-muted-foreground/55",
      );
      expect(screen.getByText("M").className, label).not.toContain("grayscale");
      expect(metaLine(card).className, label).not.toContain(
        "text-muted-foreground/40",
      );
      cleanup();
    }
  });

  it("keeps semantic colors on the workspace currently being viewed", () => {
    renderCard({
      isActive: true,
      status: "working",
      workspace: makeWorkspace({
        git_additions: 8,
        git_deletions: 3,
        pr_number: 12,
        pr_state: "open",
        pr_url: "https://example.test/pr/12",
      }),
    });

    expect(screen.getByText("M").className).not.toContain("grayscale");
    expect(screen.getByText("Ship it").className).toContain("text-foreground");
    expect(screen.getByText("+8").className).toContain("text-status-open/80");
    expect(screen.getByText("−3").className).toContain(
      "text-status-attention/80",
    );
    expect(
      screen.getByRole("button", { name: "Pull request #12 — open" }).className,
    ).toContain("text-status-open");
  });
});

describe("SidebarInboxCard — working status", () => {
  it("uses a static workspace mark instead of the thread activity orb", () => {
    const { container } = renderCard({ status: "working" });

    expect(
      container.querySelector("[data-workspace-working-icon]"),
    ).toBeInTheDocument();
    expect(container.querySelector("[data-orb-state]")).not.toBeInTheDocument();
    expect(screen.getByText("Working")).toBeInTheDocument();
  });
});

describe("SidebarInboxCard — meta line alignment", () => {
  // jsdom does not lay out, so these assert the DOM contract that *produces*
  // the alignment rather than the pixels: the git-local facts flow from the
  // left, a flex spacer absorbs the slack, and everything after it is pinned
  // right. Before this, the PR chip sat directly after the branch name, so it
  // landed at a different x on every card depending on how long that branch
  // happened to be.
  it("puts the flex spacer before the PR chip so the chip right-aligns", () => {
    const { container } = renderCard({
      workspace: makeWorkspace({
        git_branch: "a-very-long-worktree-branch-name",
        pr_number: 219,
        pr_state: "open",
        pr_url: "https://example.test/pr/219",
      }),
    });
    const children = [...metaLine(container).children];
    const spacer = children.findIndex((c) => c.className.includes("flex-1"));
    const chip = children.findIndex(
      (c) => c.getAttribute("aria-label") === "Pull request #219 — open",
    );

    expect(spacer).toBeGreaterThanOrEqual(0);
    expect(chip).toBeGreaterThan(spacer);
    // Branch text stays on the left of the spacer, where its length can only
    // push its own truncation — never the chip's position.
    expect(children[0]).toHaveTextContent("a-very-long-worktree-branch-name");
    expect(children.indexOf(children[0])).toBeLessThan(spacer);
  });

  it("keeps the git-local facts left of the spacer for every PR state", () => {
    for (const state of ["open", "closed", "draft", "merged"] as const) {
      const { container } = renderCard({
        workspace: makeWorkspace({
          git_branch: "feat/x",
          git_ahead: 2,
          git_additions: 484,
          git_deletions: 26,
          pr_number: 7,
          pr_state: state,
          pr_url: "https://example.test/pr/7",
        }),
      });
      const children = [...metaLine(container).children];
      const spacer = children.findIndex((c) => c.className.includes("flex-1"));
      const chip = children.findIndex((c) =>
        c.getAttribute("aria-label")?.startsWith("Pull request #7"),
      );

      // ahead + diff stats are git-local facts: they belong with the branch on
      // the left, so growing them shifts nothing in the right-hand column.
      expect(
        children
          .slice(0, spacer)
          .map((c) => c.textContent)
          .join(" "),
      ).toContain("↑2");
      expect(
        children
          .slice(0, spacer)
          .map((c) => c.textContent)
          .join(" "),
      ).toContain("+484");
      expect(chip).toBeGreaterThan(spacer);
      cleanup();
    }
  });

  it("reserves the trailing indicator column so a bare card still aligns", () => {
    // The reservation is a width the parent computes once per list render and
    // hands to every card, so cards with different indicator counts still
    // right-align their PR chips against one column. Its floor is the widest
    // single indicator (the 15px notification pill), not the 13px provider
    // logo — otherwise which indicator a card happens to show would shift its
    // PR chip by 2px.
    const { container } = renderCard({
      workspace: makeWorkspace({ pr_number: 1, pr_state: "open" }),
    });
    const children = [...metaLine(container).children];
    const trailing = children[children.length - 1] as HTMLElement;

    expect(trailing.style.minWidth).toBe(`${META_CLUSTER_MIN_WIDTH}px`);
    expect(trailing.className).toContain("justify-end");
    // Last child, so it — not the PR chip — owns the far-right column.
    expect(trailing.nextElementSibling).toBeNull();
  });

  it("honours the width the list reserved rather than its own content", () => {
    const { container } = renderCard({
      workspace: makeWorkspace({ pr_number: 1, pr_state: "open" }),
      metaClusterMinWidth: 46,
    });
    const children = [...metaLine(container).children];
    const trailing = children[children.length - 1] as HTMLElement;
    expect(trailing.style.minWidth).toBe("46px");
  });

  it("still renders the trailing indicators it owns", () => {
    const { container } = renderCard({
      workspace: makeWorkspace({ notification_count: 3 }),
    });
    const children = [...metaLine(container).children];
    const trailing = children[children.length - 1];
    expect(trailing).toHaveTextContent("3");
  });
});

describe("SidebarInboxCard — pin affordance", () => {
  it("offers Pin on every card, guardrail or not", () => {
    // Pin is the one action with no lifecycle guardrail: it changes where a
    // card is shown, never whether its agent is interruptible. A working or
    // blocked card that offered nothing at all would also have no way out of
    // the context menu, which is where this gesture used to be buried.
    for (const status of [
      null,
      "working",
      "permission",
      "review",
    ] as (ActivePaneStatus | null)[]) {
      renderCard({ status });
      expect(
        screen.getByRole("button", { name: 'Pin "Ship it" to top' }),
      ).toBeInTheDocument();
      cleanup();
    }
  });

  it("pins without activating the workspace", async () => {
    const { props } = renderCard({ onPin: vi.fn() });
    await userEvent.click(
      screen.getByRole("button", { name: 'Pin "Ship it" to top' }),
    );
    expect(props.onPin).toHaveBeenCalledWith("ws-1");
    expect(activateWorkspace).not.toHaveBeenCalled();
    expect(props.onSelect).not.toHaveBeenCalled();
  });

  it("turns into Unpin once the card is pinned", async () => {
    const { props } = renderCard({ pinned: true, onUnpin: vi.fn() });
    expect(
      screen.queryByRole("button", { name: 'Pin "Ship it" to top' }),
    ).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: 'Unpin "Ship it"' }),
    );
    expect(props.onUnpin).toHaveBeenCalledWith("ws-1");
    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("hides the resting pin marker under the pointer", () => {
    // The revealed cluster carries its own pin glyph, so leaving the marker up
    // would draw two pins on one row.
    const { container } = renderCard({ pinned: true });
    // An SVG node, so `className` is an SVGAnimatedString — read the attribute.
    const marker = container.querySelector('[aria-label="Pinned workspace"]')!;
    const classes = marker.getAttribute("class") ?? "";
    expect(classes).toContain("group-hover/card:hidden");
    expect(classes).toContain("group-focus-within/card:hidden");
  });

  it("keeps the state readout in place on a card that cannot settle", () => {
    // `stateKeepsPlace`: the readout yields only to the wide Snooze/Settle
    // pair. A guardrailed card reveals nothing but the narrow pin glyph, which
    // fits beside the readout — so a working agent never trades its status for
    // chrome the user cannot act on.
    const { container } = renderCard({ status: "working" });
    const state = screen.getByText("Working").closest("span")!;
    expect(state.className).not.toContain("group-hover/card:hidden");

    cleanup();
    const settleable = renderCard({ status: "review" });
    const readout = settleable.getByText("Done · review").closest("span")!;
    expect(readout.className).toContain("group-hover/card:hidden");
    expect(container).toBeTruthy();
  });
});

describe("SidebarInboxCard — running-process indicator", () => {
  it("names the port it found", () => {
    renderCard({ runningPort: 5173 });
    expect(
      screen.getByRole("img", { name: "Long-running process on :5173" }),
    ).toBeInTheDocument();
  });

  it("stays absent when nothing is listening", () => {
    renderCard();
    expect(
      screen.queryByRole("img", { name: /Long-running process/ }),
    ).toBeNull();
  });

  it("sits inside the trailing cluster, ahead of the provider marks", () => {
    const { container } = renderCard({ runningPort: 3000 });
    const children = [...metaLine(container).children];
    const trailing = children[children.length - 1];
    const indicator = screen.getByRole("img", {
      name: "Long-running process on :3000",
    });
    expect(trailing.contains(indicator)).toBe(true);
    expect(trailing.firstElementChild).toBe(indicator);
  });
});

describe("metaClusterWidth", () => {
  it("never reserves less than the widest single indicator", () => {
    expect(metaClusterWidth(0, false)).toBe(META_CLUSTER_MIN_WIDTH);
    expect(metaClusterWidth(1, false)).toBe(META_CLUSTER_MIN_WIDTH);
  });

  it("grows with the busiest card in the list, not with each card", () => {
    // 3 marks: 3×14 + 2×8 gaps.
    expect(metaClusterWidth(3, false)).toBe(58);
    // …plus the 12px run glyph and one more gap.
    expect(metaClusterWidth(3, true)).toBe(78);
  });

  it("charges no gap for a run indicator standing alone", () => {
    expect(metaClusterWidth(0, true)).toBe(META_CLUSTER_MIN_WIDTH);
  });
});

describe("SidebarInboxCard — memo boundary", () => {
  it("is wrapped in React.memo", () => {
    // The behavioral half of this guarantee — that the inbox actually hands
    // the card reference-stable props, so the boundary bails out — is asserted
    // in `sidebar-inbox-delta.test.tsx`. This is the other half: the boundary
    // exists at all. Losing the wrapper would silently restore "every card
    // re-renders on every backend tick", with no failing render assertion
    // anywhere to catch it.
    expect(
      (SidebarInboxCard as unknown as { $$typeof?: symbol }).$$typeof,
    ).toBe(Symbol.for("react.memo"));
  });
});

describe("SidebarInboxCard — working duration", () => {
  it("shows the elapsed working time beside the status", () => {
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "working", at: 0 } },
    });

    renderCard({ status: "working", now: 12 * 60_000 });

    expect(screen.getByRole("status")).toHaveTextContent("Working");
    expect(screen.getByText("12m")).toHaveClass("font-mono", "tabular-nums");
    expect(screen.getByText("12m")).toHaveAttribute("aria-hidden", "true");
  });

  it("does not reuse a timestamp from another agent state", () => {
    useSidebarDensityStore.setState({
      statusSince: { "ws-1": { status: "permission", at: 0 } },
    });

    renderCard({ status: "working", now: 12 * 60_000 });

    expect(screen.queryByText("12m")).not.toBeInTheDocument();
  });
});

describe("SidebarInboxCard — monitoring status", () => {
  // Monitoring is a live state, so its readout is exempt from the card's
  // recede: colour and dot are there at rest, no hover needed.
  it("keeps the Monitoring tone on a receded background card", () => {
    const { card } = renderCard({ status: "monitoring" as ActivePaneStatus });
    const label = screen.getByText("Monitoring");
    expect(label.className).toContain("text-status-monitoring");
    expect(label.className).toContain("font-semibold");
    expect(label.className).not.toContain("text-muted-foreground");
    const dot = [...card.querySelectorAll("span")].find((element) =>
      element.className.includes("bg-status-monitoring"),
    );
    expect(dot).toBeDefined();
  });

  // The whole point of a separate status: monitoring is calm. A pulsing dot
  // is this app's "look at me" vocabulary and belongs to `permission` alone.
  it("renders a steady dot with no pulse", () => {
    const { card } = renderCard({ status: "monitoring" as ActivePaneStatus });
    const dot = [...card.querySelectorAll("span")].find((element) =>
      element.className.includes("bg-status-monitoring"),
    );
    expect(dot).toBeDefined();
    expect(dot?.className).not.toContain("animate");
  });

  // Settle/Snooze are guarded only against live and blocked agents. A
  // workspace left babysitting CI is exactly the kind of thing a user parks.
  it("still offers Settle and Snooze", () => {
    renderCard({ status: "monitoring" as ActivePaneStatus });
    expect(
      screen.getByRole("button", { name: 'Settle "Ship it"' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: 'Snooze "Ship it"' }),
    ).toBeInTheDocument();
  });
});
