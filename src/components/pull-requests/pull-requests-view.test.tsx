/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { PrRow } from "@/lib/pr-overview";

// ── The page, with its two neighbours stubbed ──
//
// What is under test here is the page's own two jobs: deciding when
// Escape means "leave", and keeping a tab's detail on screen after the
// list stops carrying its row. The overview fetch and the detail column
// are both exercised by their own suites, and mounting them here would
// pull in half the review surface to answer neither question.

const overview: {
  rows: PrRow[];
  viewerByRoot: Map<string, string | null>;
  stateFilter?: string;
} = { rows: [], viewerByRoot: new Map() };

const lookup = vi.hoisted(() => {
  const ui = {
    pendingPrSelection: null as { projectRoot: string; number: number; url?: string } | null,
  };
  const clearPendingPrSelection = vi.fn(() => {
    ui.pendingPrSelection = null;
  });
  const queryClient = { fetchQuery: vi.fn() };
  return { ui, clearPendingPrSelection, queryClient };
});

vi.mock("@tanstack/react-query", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-query")>()),
  useQueryClient: () => lookup.queryClient,
}));

vi.mock("@/lib/pr-overview-query", () => ({
  prOverviewKey: (root: string) => ["prs", "overview", root],
  prHistoryKey: (root: string, state: string) => ["prs", "history", root, state],
  usePrOverview: (_enabled: boolean, stateFilter: string) => {
    overview.stateFilter = stateFilter;
    return {
    rows: overview.rows,
    viewerByRoot: overview.viewerByRoot,
    failures: [],
    roots: [],
    updatedAt: Date.now(),
    carried: false,
    carriedAt: null,
    allRootsFailed: false,
    refreshFailed: false,
    isLoading: false,
    refresh: vi.fn(),
    };
  },
}));

vi.mock("./pr-detail-column", () => ({
  PrDetailColumn: ({ row }: { row: PrRow }) => (
    <div data-testid="pr-detail">
      {row.title} · {row.state ?? "OPEN"}
    </div>
  ),
}));

vi.mock("@/components/layout/window-chrome", () => ({
  WindowChrome: () => null,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

const mockSetShowPullRequests = vi.fn();
vi.mock("@/stores/ui-store", () => {
  const state = () => ({
    setShowPullRequests: mockSetShowPullRequests,
    pendingPrSelection: lookup.ui.pendingPrSelection,
    clearPendingPrSelection: lookup.clearPendingPrSelection,
    markPrBadgeSeen: vi.fn(),
  });
  return {
    useUIStore: Object.assign(
      (selector: (s: Record<string, unknown>) => unknown) => selector(state()),
      { getState: state },
    ),
  };
});

vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (s: Record<string, unknown>) => unknown) =>
    selector({ appState: { workspaces: [] } }),
}));

import { openUrl } from "@tauri-apps/plugin-opener";
import { PullRequestsView } from "./pull-requests-view";

const ROOT = "/home/dev/projects/codemux";

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
    updated_at: new Date().toISOString(),
    url: `https://github.com/example/codemux/pull/${over.number}`,
    projectRoot: ROOT,
    repo: "example/codemux",
    providerKind: "github",
    ...over,
  };
}

function renderView(rows: PrRow[]) {
  overview.rows = rows;
  overview.viewerByRoot = new Map([[ROOT, "mock-dev"]]);
  const utils = render(<PullRequestsView />);
  return {
    ...utils,
    setRows: (next: PrRow[]) => {
      overview.rows = next;
      utils.rerender(<PullRequestsView />);
    },
  };
}

beforeEach(() => {
  mockSetShowPullRequests.mockClear();
  lookup.ui.pendingPrSelection = null;
  lookup.clearPendingPrSelection.mockClear();
  lookup.queryClient.fetchQuery.mockReset();
  vi.mocked(openUrl).mockClear();
});

afterEach(() => {
  cleanup();
});

describe("Escape on the Pull Requests page", () => {
  it("closes the page from the list", async () => {
    const user = userEvent.setup();
    renderView([row({ number: 1 })]);

    // Focused explicitly: the list container is what holds focus once you
    // click or tab into the results, and jsdom does not move focus on a
    // click. Escape has to keep working from there — the container is a
    // `role="listbox"`, and treating that role as an overlay switched the
    // page's own Escape off for the most ordinary position on the page.
    screen.getByTestId("pr-list-rows").focus();
    expect(document.activeElement).toBe(screen.getByTestId("pr-list-rows"));

    await user.keyboard("{Escape}");
    expect(mockSetShowPullRequests).toHaveBeenCalledWith(false);
  });

  it("leaves the page alone while something is being typed into", async () => {
    // A reply box, the line composer, the submit sheet's body: Escape
    // there means "stop editing", and closing the whole destination
    // destroys the text it was asking about.
    const user = userEvent.setup();
    renderView([row({ number: 1 })]);

    const textarea = document.createElement("textarea");
    document.body.appendChild(textarea);
    textarea.focus();

    await user.keyboard("{Escape}");
    expect(mockSetShowPullRequests).not.toHaveBeenCalled();
    textarea.remove();
  });

  it("leaves the page alone while an overlay owns the key", async () => {
    // Radix does not `preventDefault` when a dialog dismisses itself, so
    // the page has to recognise the overlay rather than wait to be told.
    const user = userEvent.setup();
    renderView([row({ number: 1 })]);

    const sheet = document.createElement("div");
    sheet.setAttribute("role", "dialog");
    sheet.setAttribute("data-state", "open");
    sheet.tabIndex = -1;
    document.body.appendChild(sheet);
    sheet.focus();

    await user.keyboard("{Escape}");
    expect(mockSetShowPullRequests).not.toHaveBeenCalled();
    sheet.remove();
  });

  it("leaves the page alone when something closer already handled it", async () => {
    renderView([row({ number: 1 })]);

    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    event.preventDefault();
    window.dispatchEvent(event);

    expect(mockSetShowPullRequests).not.toHaveBeenCalled();
  });
});

describe("a pull request that leaves the list", () => {
  it("keeps the open tab's detail on screen after it is merged", async () => {
    // Merging drops the row out of the default "open" filter on the very
    // next refresh. Resolving the tab from the filtered list alone
    // emptied the page at the exact moment the user was waiting to see
    // that it had worked.
    const user = userEvent.setup();
    const open = row({ number: 1, state: "OPEN" });
    const { setRows } = renderView([open]);

    await user.click(screen.getByText("pull request 1"));
    expect(await screen.findByTestId("pr-detail")).toHaveTextContent("OPEN");

    // The refresh after a merge: the row is simply gone from "open".
    setRows([]);

    await waitFor(() =>
      expect(screen.getByTestId("pr-detail")).toBeInTheDocument(),
    );
    expect(screen.queryByText(/Pick a pull request/)).toBeNull();
  });
});

describe("a link to a pull request the list doesn't hold", () => {
  const url = "https://github.com/example/codemux/pull/353";

  it("widens to the history when that is where it lives", async () => {
    // Already merged: absent from the open list, present in the history.
    lookup.queryClient.fetchQuery
      .mockResolvedValueOnce({ items: [], viewer: "mock-dev" })
      .mockResolvedValueOnce([{ number: 353 }]);
    lookup.ui.pendingPrSelection = { projectRoot: ROOT, number: 353, url };

    renderView([row({ number: 1 })]);

    await waitFor(() => expect(overview.stateFilter).toBe("all"));
    expect(openUrl).not.toHaveBeenCalled();
    expect(lookup.clearPendingPrSelection).not.toHaveBeenCalled();
  });

  it("hands the link to the browser once neither list has it", async () => {
    lookup.queryClient.fetchQuery
      .mockResolvedValueOnce({ items: [], viewer: "mock-dev" })
      .mockResolvedValueOnce([]);
    lookup.ui.pendingPrSelection = { projectRoot: ROOT, number: 353, url };

    renderView([row({ number: 1 })]);

    await waitFor(() => expect(openUrl).toHaveBeenCalledWith(url));
    expect(lookup.clearPendingPrSelection).toHaveBeenCalled();
    expect(lookup.queryClient.fetchQuery).toHaveBeenCalledTimes(2);
  });
});
