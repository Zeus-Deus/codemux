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
} = { rows: [], viewerByRoot: new Map() };

vi.mock("@/lib/pr-overview-query", () => ({
  usePrOverview: () => ({
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
  }),
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
vi.mock("@/stores/ui-store", () => ({
  useUIStore: (selector: (s: Record<string, unknown>) => unknown) =>
    selector({
      setShowPullRequests: mockSetShowPullRequests,
      pendingPrSelection: null,
      clearPendingPrSelection: vi.fn(),
      markPrBadgeSeen: vi.fn(),
    }),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (s: Record<string, unknown>) => unknown) =>
    selector({ appState: { workspaces: [] } }),
}));

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
});

afterEach(() => {
  cleanup();
});

describe("Escape on the Pull Requests page", () => {
  it("closes the page from the list", async () => {
    const user = userEvent.setup();
    renderView([row({ number: 1 })]);

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
