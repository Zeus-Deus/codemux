import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GitHubIssue, PullRequestInfo } from "@/tauri/types";
import { IssuePickerPanel } from "./issue-picker";
import { PrPickerPanel } from "./pr-picker";

const commands = vi.hoisted(() => ({
  issues: vi.fn<(...args: unknown[]) => Promise<GitHubIssue[]>>(),
  prs: vi.fn<(...args: unknown[]) => Promise<PullRequestInfo[]>>(),
  directPr: vi.fn<(...args: unknown[]) => Promise<PullRequestInfo>>(),
}));

vi.mock("@/tauri/commands", () => ({
  listGithubIssues: commands.issues,
  listGithubIssuesByPath: commands.issues,
  listPullRequests: commands.prs,
  getGithubPrByPath: commands.directPr,
}));

function issue(number: number): GitHubIssue {
  return { number, title: `Result ${number}`, state: "Open", labels: [], assignees: [], url: "", body: null, comments: [], totalComments: 0, updatedAt: null };
}

function pr(number: number): PullRequestInfo {
  return {
    number, title: `Result ${number}`, state: "OPEN", url: "", head_branch: null,
    base_branch: null, is_draft: false, mergeable: null, additions: null,
    deletions: null, review_decision: null, checks_passing: null, updated_at: null,
    created_at: null, body: null, comments: [], totalComments: 0, author: null,
    head_ref_oid: null, head_repository_owner: null, merge_state_status: null,
    changed_files: null, merged_by: null, merged_at: null, review_requests: [], latest_reviews: [],
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe.each(["issue", "PR"] as const)("%s picker search lifecycle", (kind) => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetAllMocks();
    commands.issues.mockResolvedValue([]);
    commands.prs.mockResolvedValue([]);
    commands.directPr.mockReturnValue(new Promise(() => {}));
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  async function mountPanel() {
    const onSelect = vi.fn();
    const onClose = vi.fn();
    const panel = (open = true, projectPath = "/repo") => kind === "issue"
      ? <IssuePickerPanel projectPath={projectPath} open={open} onSelect={onSelect} onClose={onClose} />
      : <PrPickerPanel projectPath={projectPath} open={open} onSelect={onSelect} onClose={onClose} />;
    const result = render(panel());
    await act(async () => {});
    const view = within(result.container);
    const search = (query: string) => fireEvent.change(view.getByRole("textbox"), { target: { value: query } });
    const pendingSearch = (number: number) => {
      const pending = deferred<void>();
      if (kind === "issue") commands.issues.mockReturnValueOnce(pending.promise.then(() => [issue(number)]));
      else commands.directPr.mockReturnValueOnce(pending.promise.then(() => pr(number)));
      return () => act(async () => { pending.resolve(); });
    };
    const searchCalls = () => kind === "issue"
      ? commands.issues.mock.calls.filter((args) => args[1] !== undefined)
      : commands.directPr.mock.calls;
    return { ...result, view, panel, search, pendingSearch, searchCalls, onSelect };
  }

  const debounce = () => act(async () => { await vi.advanceTimersByTimeAsync(300); });

  it("keeps the latest query when search responses finish out of order", async () => {
    const { view, search, pendingSearch } = await mountPanel();
    const resolveFirst = pendingSearch(11);
    search("11");
    await debounce();
    const resolveLatest = pendingSearch(22);
    search("22");
    await debounce();
    await resolveLatest();
    await resolveFirst();
    expect(view.getByText("Result 22")).toBeInTheDocument();
    expect(view.queryByText("Result 11")).not.toBeInTheDocument();
  });

  it("does not restore a search result after the query is cleared", async () => {
    const { view, search, pendingSearch } = await mountPanel();
    const resolveSearch = pendingSearch(11);
    search("11");
    await debounce();
    search("");
    await resolveSearch();
    expect(view.queryByText("Result 11")).not.toBeInTheDocument();
  });

  it("removes the previous server result while a new query is debouncing", async () => {
    const { view, search, pendingSearch } = await mountPanel();
    const resolveSearch = pendingSearch(11);
    search("11");
    await debounce();
    await resolveSearch();
    expect(view.getByText("Result 11")).toBeInTheDocument();
    search("22");
    expect(view.queryByText("Result 11")).not.toBeInTheDocument();
  });

  it("cancels a pending search when unmounted", async () => {
    const { search, searchCalls, unmount } = await mountPanel();
    search("11");
    unmount();
    await debounce();
    expect(searchCalls()).toHaveLength(0);
  });

  it("cancels a pending search when the picker closes", async () => {
    const { search, searchCalls, rerender, panel } = await mountPanel();
    search("11");
    rerender(panel(false));
    await debounce();
    expect(searchCalls()).toHaveLength(0);
  });

  it("ignores results from the previous project", async () => {
    const { view, search, pendingSearch, rerender, panel } = await mountPanel();
    const resolveSearch = pendingSearch(11);
    search("11");
    await debounce();
    rerender(panel(true, "/other-repo"));
    await act(async () => {});
    await resolveSearch();
    expect(view.queryByText("Result 11")).not.toBeInTheDocument();
  });
});
