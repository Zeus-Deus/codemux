/**
 * What the two search dialogs do with a query: where a picked hit lands,
 * how a pending search looks, and what they say about caps, errors and
 * their own toggles.
 */
import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { useUIStore } from "@/stores/ui-store";
import { useEditorStore } from "@/stores/editor-store";
import type { SearchResult } from "@/tauri/types";

const { commands, openEditorTab, workspace } = vi.hoisted(() => ({
  commands: {
    searchFileNames: vi.fn(),
    searchInFiles: vi.fn(),
    getGitStatus: vi.fn(),
  },
  openEditorTab: vi.fn(),
  workspace: {
    workspace_id: "ws-1",
    tabs: [{ tab_id: "ed-1", kind: "editor", title: "open.ts" }],
  },
}));

vi.mock("@/tauri/commands", () => commands);
vi.mock("@/lib/open-editor-tab", () => ({ openEditorTab }));
vi.mock("@/lib/open-right-panel-doc", () => ({ openRightPanelDoc: vi.fn() }));
vi.mock("@/stores/app-store", () => ({
  selectActiveWorkspaceId: () => "ws-1",
  useActiveWorkspaceCwd: () => "/repo",
  useAppStore: Object.assign(
    vi.fn(() => null),
    { getState: () => ({ appState: { workspaces: [workspace] } }) },
  ),
}));

import { FileSearchDialog, FILE_SEARCH_LIMIT } from "./file-search-dialog";
import { ContentSearchDialog, CONTENT_SEARCH_LIMIT } from "./content-search-dialog";

function hit(over: Partial<SearchResult> = {}): SearchResult {
  return {
    file_path: "/repo/src/a.ts",
    line_number: 42,
    line_content: "const needle = 1;",
    match_start: 6,
    match_end: 12,
    ...over,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

function type(value: string) {
  fireEvent.change(screen.getByRole("textbox"), { target: { value } });
}

beforeEach(() => {
  vi.clearAllMocks();
  commands.searchFileNames.mockResolvedValue([]);
  commands.searchInFiles.mockResolvedValue([]);
  commands.getGitStatus.mockResolvedValue([]);
  openEditorTab.mockResolvedValue("tab-1");
  useEditorStore.setState({ tabs: { "ed-1": { filePath: "/repo/src/open.ts", baselineContent: "", isDirty: false } } });
  useUIStore.setState({ showFileSearch: false, showContentSearch: false, fileSearchTarget: "editor" });
});
afterEach(cleanup);

describe("content search", () => {
  it("opens a picked match at its line with the hit selected", async () => {
    commands.searchInFiles.mockResolvedValue([hit()]);
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);

    type("needle");
    await screen.findByText("needle");
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });

    await waitFor(() => expect(openEditorTab).toHaveBeenCalledWith("ws-1", workspace.tabs, "/repo/src/a.ts"));
    await waitFor(() =>
      expect(useEditorStore.getState().getTab("tab-1")?.revealRequest).toMatchObject({
        line: 42,
        column: 7,
        endColumn: 13,
      }),
    );
  });

  it("shows file paths relative to the workspace", async () => {
    commands.searchInFiles.mockResolvedValue([hit()]);
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    type("needle");
    expect(await screen.findByText("src/a.ts")).toBeInTheDocument();
  });

  it("says why a search failed instead of claiming no results", async () => {
    commands.searchInFiles.mockRejectedValue("unclosed group");
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    type("(");
    expect(await screen.findByRole("alert")).toHaveTextContent("unclosed group");
    expect(screen.queryByText("No results found")).toBeNull();
  });

  it("exposes toggle state and flips it with Alt+C / Alt+R", async () => {
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    const matchCase = screen.getByRole("button", { name: "Match case" });
    const regex = screen.getByRole("button", { name: "Use regular expression" });
    expect(matchCase).toHaveAttribute("aria-pressed", "false");
    expect(regex).toHaveAttribute("aria-pressed", "false");

    const input = screen.getByRole("textbox");
    fireEvent.keyDown(input, { key: "c", code: "KeyC", altKey: true });
    fireEvent.keyDown(input, { key: "r", code: "KeyR", altKey: true });
    expect(matchCase).toHaveAttribute("aria-pressed", "true");
    expect(regex).toHaveAttribute("aria-pressed", "true");

    type("ne+dle");
    await waitFor(() =>
      expect(commands.searchInFiles).toHaveBeenCalledWith("/repo", "ne+dle", true, true, CONTENT_SEARCH_LIMIT),
    );
  });

  it("says when results stopped at the cap", async () => {
    commands.searchInFiles.mockResolvedValue(
      Array.from({ length: CONTENT_SEARCH_LIMIT }, (_, i) => hit({ line_number: i + 1 })),
    );
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);
    type("needle");
    expect(await screen.findByText(/Showing the first 100/)).toBeInTheDocument();
  });

  it("keeps stale rows in place while the next query runs, and drops a late older response", async () => {
    const first = deferred<SearchResult[]>();
    const second = deferred<SearchResult[]>();
    commands.searchInFiles.mockResolvedValueOnce([hit({ line_content: "first hit" , match_start: 0, match_end: 5 })]);
    useUIStore.setState({ showContentSearch: true });
    render(<ContentSearchDialog />);

    type("first");
    await screen.findByText("hit", { exact: false });

    commands.searchInFiles.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    type("firs");
    // Pending: the old row stays put and the spinner sits in the input.
    expect(screen.getByLabelText("Searching")).toBeInTheDocument();
    expect(screen.getByText("hit", { exact: false })).toBeInTheDocument();
    await waitFor(() => expect(commands.searchInFiles).toHaveBeenCalledTimes(2));

    type("fir");
    await waitFor(() => expect(commands.searchInFiles).toHaveBeenCalledTimes(3));
    second.resolve([hit({ line_content: "newest", match_start: 0, match_end: 3 })]);
    await screen.findByText("est", { exact: false });
    first.resolve([hit({ line_content: "outdated", match_start: 0, match_end: 3 })]);
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByText("dated", { exact: false })).toBeNull();
  });
});

describe("file search", () => {
  it("suggests open and changed files before anything is typed", async () => {
    commands.getGitStatus.mockResolvedValue([
      { path: "src/changed.ts", status: "modified" },
      { path: "src/gone.ts", status: "deleted" },
    ]);
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);

    expect(screen.getByText("Open in editor")).toBeInTheDocument();
    expect(screen.getByText("open.ts")).toBeInTheDocument();
    expect(await screen.findByText("Changed")).toBeInTheDocument();
    expect(screen.getByText("changed.ts")).toBeInTheDocument();
    expect(screen.queryByText("gone.ts")).toBeNull();
  });

  it("highlights the matched part of the file name", async () => {
    commands.searchFileNames.mockResolvedValue(["src/components/right-panel.tsx"]);
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);
    type("panel");
    const mark = await screen.findByText("panel");
    expect(mark.tagName).toBe("MARK");
  });

  it("says when results stopped at the cap", async () => {
    commands.searchFileNames.mockResolvedValue(
      Array.from({ length: FILE_SEARCH_LIMIT }, (_, i) => `src/file-${i}.ts`),
    );
    useUIStore.setState({ showFileSearch: true });
    render(<FileSearchDialog />);
    type("file");
    expect(await screen.findByText(/Showing the first 20/)).toBeInTheDocument();
  });
});
