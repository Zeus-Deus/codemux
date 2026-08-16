/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockCheckOutPr = vi.fn().mockResolvedValue(undefined);
vi.mock("@/lib/pr-checkout", () => ({
  checkOutPr: (...a: unknown[]) => mockCheckOutPr(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

import { PrList } from "./pr-list";
import { rowKey, type PrRow } from "@/lib/pr-overview";

const ROOT = "/home/dev/projects/codemux";
const GITLAB_ROOT = "/home/dev/projects/vexis";
const VIEWER = "mock-dev";

function row(over: Partial<PrRow> & { number: number }): PrRow {
  return {
    title: `pull request ${over.number}`,
    author: "juliusm",
    head_branch: `branch/${over.number}`,
    is_draft: false,
    additions: 10,
    deletions: 2,
    review_decision: null,
    checks: "passing",
    review_requested_from: [],
    updated_at: new Date(Date.now() - over.number * 60_000).toISOString(),
    url: `https://github.com/example/codemux/pull/${over.number}`,
    projectRoot: ROOT,
    repo: "example/codemux",
    providerKind: "github",
    ...over,
  };
}

const viewers = new Map<string, string | null>([
  [ROOT, VIEWER],
  [GITLAB_ROOT, "mock-glab"],
]);

function renderList(over: Partial<Parameters<typeof PrList>[0]> = {}) {
  const props = {
    rows: [] as PrRow[],
    viewerByRoot: viewers,
    workspaceByBranch: new Map<string, string>(),
    failures: [],
    hostCount: 2,
    updatedAt: Date.now(),
    isLoading: false,
    selectedKey: null,
    stateFilter: "open" as const,
    onStateFilter: vi.fn(),
    onSelect: vi.fn(),
    onOpenDetail: vi.fn(),
    onRefresh: vi.fn(),
    ...over,
  };
  const utils = render(<PrList {...props} />);
  return {
    ...utils,
    props,
    rerender: (next: Partial<typeof props>) =>
      utils.rerender(<PrList {...props} {...next} />),
  };
}

/** The rows in the order the DOM actually has them. */
function domOrder(): number[] {
  return screen
    .getAllByRole("option")
    .map((el) => Number(el.getAttribute("data-testid")?.split("-").pop()));
}

beforeEach(() => {
  mockCheckOutPr.mockClear();
});
afterEach(cleanup);

describe("groups", () => {
  const rows = [
    row({ number: 10, review_requested_from: [VIEWER] }),
    row({ number: 20, author: VIEWER }),
    row({ number: 30 }),
  ];

  it("renders the two groups that want something from you, and folds Watching", () => {
    renderList({ rows });

    expect(screen.getByTestId("pr-group-review")).toHaveTextContent("Needs your review");
    expect(screen.getByTestId("pr-group-yours")).toHaveTextContent("Yours");

    const watching = screen.getByTestId("pr-group-watching-toggle");
    expect(watching).toHaveAttribute("aria-expanded", "false");
    expect(watching).toHaveTextContent("1");
    // Folded means not rendered: #30 is the only watching row.
    expect(domOrder()).toEqual([10, 20]);
  });

  it("unfolds Watching on click", async () => {
    renderList({ rows });
    await userEvent.click(screen.getByTestId("pr-group-watching-toggle"));
    expect(domOrder()).toEqual([10, 20, 30]);
  });

  it("opens Watching by itself when it is the only populated group", async () => {
    // Nothing is attributed to the viewer — a folded header would be
    // the entire page, so the fold follows the content instead.
    renderList({ rows: [row({ number: 30 }), row({ number: 40 })] });

    expect(screen.getByTestId("pr-group-watching-toggle")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(domOrder()).toEqual([30, 40]);

    // An explicit collapse still wins over the default.
    await userEvent.click(screen.getByTestId("pr-group-watching-toggle"));
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });
});

describe("rule 03 — order freezes while you are in the list", () => {
  const before = [
    row({ number: 10, author: VIEWER }),
    row({ number: 20, author: VIEWER }),
    row({ number: 30, author: VIEWER }),
  ];
  // A poll that would put #30 on top — the fixture ages by number, so a
  // fresher timestamp is what a real update looks like.
  const after = [
    { ...before[2], updated_at: new Date().toISOString() },
    before[0],
    before[1],
  ];

  it("holds the order under the pointer and marks what moved", () => {
    const { rerender } = renderList({ rows: before });
    expect(domOrder()).toEqual([10, 20, 30]);

    fireEvent.mouseEnter(screen.getByTestId("pr-list-rows"));
    rerender({ rows: after });

    expect(domOrder()).toEqual([10, 20, 30]);
    expect(screen.getAllByTestId("pr-row-moved").length).toBeGreaterThan(0);
  });

  it("applies the held order once the pointer leaves", () => {
    const { rerender } = renderList({ rows: before });
    const list = screen.getByTestId("pr-list-rows");

    fireEvent.mouseEnter(list);
    rerender({ rows: after });
    expect(domOrder()).toEqual([10, 20, 30]);

    fireEvent.mouseLeave(list);
    expect(domOrder()).toEqual([30, 10, 20]);
    expect(screen.queryByTestId("pr-row-moved")).toBeNull();
  });

  it("keeps updating what a row says while its position is held", () => {
    const { rerender } = renderList({ rows: before });
    fireEvent.mouseEnter(screen.getByTestId("pr-list-rows"));

    rerender({
      rows: [
        { ...before[0], review_decision: "CHANGES_REQUESTED" },
        before[1],
        before[2],
      ],
    });

    expect(domOrder()).toEqual([10, 20, 30]);
    expect(screen.getByTestId("pr-row-state-label")).toHaveTextContent(
      "changes requested",
    );
  });

  it("applies the order when a row is chosen — a deliberate action", async () => {
    const onSelect = vi.fn();
    const { rerender } = renderList({ rows: before, onSelect });
    const list = screen.getByTestId("pr-list-rows");

    fireEvent.mouseEnter(list);
    rerender({ rows: after, onSelect });
    expect(domOrder()).toEqual([10, 20, 30]);

    await userEvent.click(screen.getByTestId(`pr-row-${ROOT}-20`));
    expect(onSelect).toHaveBeenCalled();
    expect(domOrder()).toEqual([30, 10, 20]);
  });
});

describe("rows", () => {
  it("labels changes requested, ready to merge and drafts", () => {
    renderList({
      rows: [
        row({ number: 10, author: VIEWER, review_decision: "CHANGES_REQUESTED" }),
        row({ number: 20, author: VIEWER, review_decision: "APPROVED" }),
        row({ number: 30, author: VIEWER, is_draft: true }),
      ],
    });

    const first = screen.getByTestId(`pr-row-${ROOT}-10`);
    expect(within(first).getByTestId("pr-row-state-label")).toHaveTextContent(
      "changes requested",
    );
    const second = screen.getByTestId(`pr-row-${ROOT}-20`);
    expect(within(second).getByTestId("pr-row-state-label")).toHaveTextContent(
      "ready to merge",
    );
    expect(screen.getByTestId(`pr-row-${ROOT}-30`)).toHaveTextContent("Draft");
  });

  it("does not call a red PR ready to merge", () => {
    renderList({
      rows: [row({ number: 10, author: VIEWER, review_decision: "APPROVED", checks: "failing" })],
    });
    expect(screen.queryByTestId("pr-row-state-label")).toBeNull();
  });

  it("uses the provider's sigil", () => {
    renderList({
      rows: [
        row({ number: 10, author: VIEWER }),
        row({
          number: 143,
          projectRoot: GITLAB_ROOT,
          providerKind: "gitlab",
          author: "mock-glab",
        }),
      ],
    });
    expect(screen.getByTestId(`pr-row-${ROOT}-10`)).toHaveTextContent("#10");
    const mr = screen.getByTestId(`pr-row-${GITLAB_ROOT}-143`);
    expect(mr).toHaveTextContent("!143");
    expect(within(mr).getByTestId("host-mark-gitlab")).toBeTruthy();
  });

  it("offers Check out, and Switch when a workspace already has the branch", async () => {
    const withWorkspace = new Map([[`${ROOT}\0branch/20`, "ws-7"]]);
    renderList({
      rows: [row({ number: 10, author: VIEWER }), row({ number: 20, author: VIEWER })],
      workspaceByBranch: withWorkspace,
    });

    const plain = within(screen.getByTestId(`pr-row-${ROOT}-10`)).getByTestId(
      "pr-row-checkout",
    );
    expect(plain).toHaveTextContent("Check out");

    const existing = screen.getByTestId(`pr-row-${ROOT}-20`);
    expect(within(existing).getByTestId("pr-row-checkout")).toHaveTextContent("Switch");
    expect(existing).toHaveTextContent("checked out");

    await userEvent.click(plain);
    expect(mockCheckOutPr).toHaveBeenCalledWith({
      projectRoot: ROOT,
      headBranch: "branch/10",
      prNumber: 10,
      existingWorkspaceId: null,
    });
  });

  it("marks the selected row and only that one", () => {
    const rows = [row({ number: 10, author: VIEWER }), row({ number: 20, author: VIEWER })];
    renderList({ rows, selectedKey: rowKey(rows[1]) });
    expect(screen.getByTestId(`pr-row-${ROOT}-20`)).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByTestId(`pr-row-${ROOT}-10`)).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });
});

describe("search", () => {
  const rows = [
    row({ number: 10, author: VIEWER, title: "fix windows shutdown", checks: "failing" }),
    row({ number: 20, author: VIEWER, title: "docs tidy", is_draft: true }),
    row({ number: 30, author: VIEWER, title: "draft windows work", is_draft: true, checks: "failing" }),
  ];

  it("filters by is:draft", async () => {
    renderList({ rows });
    await userEvent.type(screen.getByTestId("pr-search"), "is:draft");
    expect(domOrder()).toEqual([20, 30]);
  });

  it("filters by ci:failing", async () => {
    renderList({ rows });
    await userEvent.type(screen.getByTestId("pr-search"), "ci:failing");
    expect(domOrder()).toEqual([10, 30]);
  });

  it("combines a token with free text", async () => {
    renderList({ rows });
    await userEvent.type(screen.getByTestId("pr-search"), "is:draft windows");
    expect(domOrder()).toEqual([30]);
  });

  it("says so — and how — when nothing matches", async () => {
    renderList({ rows });
    await userEvent.type(screen.getByTestId("pr-search"), "nonesuch");
    expect(screen.getByText("Nothing matches that search.")).toBeInTheDocument();
    await userEvent.click(screen.getByText("Clear the search"));
    expect(domOrder()).toEqual([10, 20, 30]);
  });
});

describe("keyboard", () => {
  const rows = [
    row({ number: 10, author: VIEWER }),
    row({ number: 20, author: VIEWER }),
  ];

  it("moves the selection with the arrow keys and opens with enter", () => {
    const onSelect = vi.fn();
    const onOpenDetail = vi.fn();
    renderList({ rows, onSelect, onOpenDetail, selectedKey: rowKey(rows[0]) });
    const list = screen.getByTestId("pr-list-rows");

    fireEvent.keyDown(list, { key: "ArrowDown" });
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ number: 20 }));

    fireEvent.keyDown(list, { key: "Enter" });
    expect(onOpenDetail).toHaveBeenCalled();
  });

  it("stops at the ends rather than wrapping", () => {
    const onSelect = vi.fn();
    renderList({ rows, onSelect, selectedKey: rowKey(rows[0]) });
    fireEvent.keyDown(screen.getByTestId("pr-list-rows"), { key: "ArrowUp" });
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ number: 10 }));
  });
});

describe("footer and empty states", () => {
  it("reports unreachable repositories without blanking the list", () => {
    renderList({
      rows: [row({ number: 10, author: VIEWER })],
      failures: [
        {
          root: { path: "/home/dev/projects/old-blog", providerKind: "github", name: "old-blog" },
          message: "could not resolve host",
        },
      ],
    });
    expect(screen.getByTestId("pr-list-failures")).toHaveTextContent("1 unreachable");
    expect(domOrder()).toEqual([10]);
  });

  it("says what to do next when there is nothing to show", () => {
    renderList({ rows: [], hostCount: 0 });
    expect(screen.getByText("No projects open.")).toBeInTheDocument();
  });

  it("distinguishes no projects from no open pull requests", () => {
    renderList({ rows: [], hostCount: 2 });
    expect(screen.getByText("No open pull requests.")).toBeInTheDocument();
  });
});

/**
 * Task: the hover action must not move anything.
 *
 * jsdom applies no stylesheets, so a pixel measurement here would read
 * zero either way and prove nothing. What it *can* prove is the stronger
 * property the fix actually relies on: hovering changes no DOM at all.
 * The action's slot is present and identically sized at rest, and the
 * button inside it is revealed by `visibility`, which by definition
 * cannot reflow its siblings — as opposed to the old `display` toggle,
 * which inserted a box into the title row and pushed the state label.
 */
describe("row hover does not reflow the row", () => {
  const hoverRow = row({
    number: 41,
    author: VIEWER,
    review_decision: "APPROVED",
    review_requested_from: [VIEWER],
  });

  /** The tag/testid shape of the title row, as a comparable snapshot. */
  function titleRowShape() {
    const label = screen.getByTestId("pr-row-state-label");
    const titleRow = label.parentElement!;
    return Array.from(titleRow.children).map(
      (child) =>
        `${child.tagName}:${child.getAttribute("data-testid") ?? ""}:${child.className}`,
    );
  }

  it("holds the action's slot open at rest, so the state label never moves", async () => {
    const user = userEvent.setup();
    renderList({ rows: [hoverRow] });

    // The slot is in the tree before anything is hovered.
    const slot = screen.getByTestId("pr-row-action-slot");
    expect(slot).toBeInTheDocument();
    expect(slot.className).toContain("w-[78px]");

    const atRest = titleRowShape();
    const labelIndexAtRest = Array.from(
      screen.getByTestId("pr-row-state-label").parentElement!.children,
    ).indexOf(screen.getByTestId("pr-row-state-label"));

    await user.hover(screen.getByTestId(`pr-row-${ROOT}-41`));

    // Same children, same order, same classes: there is no geometry
    // change for the label to be pushed by.
    expect(titleRowShape()).toEqual(atRest);
    expect(
      Array.from(
        screen.getByTestId("pr-row-state-label").parentElement!.children,
      ).indexOf(screen.getByTestId("pr-row-state-label")),
    ).toBe(labelIndexAtRest);
  });

  it("reveals the action with visibility rather than display", () => {
    renderList({ rows: [hoverRow] });
    const action = screen.getByTestId("pr-row-checkout");

    expect(action.className).toContain("invisible");
    expect(action.className).toContain("group-hover:visible");
    // The old behaviour, explicitly ruled out: `hidden` + `group-hover:block`
    // took the button out of flow and put it back on hover.
    expect(action.className).not.toContain("group-hover:block");
    expect(action.className.split(/\s+/)).not.toContain("hidden");
  });
});
