/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockGetGitStatus = vi.fn();
const mockGetGitDiff = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getGitStatus: (...a: unknown[]) => mockGetGitStatus(...a),
  getGitDiff: (...a: unknown[]) => mockGetGitDiff(...a),
  getBaseBranchDiff: vi.fn().mockResolvedValue({ files: [], merge_base_commit: "" }),
  getBaseBranchFileDiff: vi.fn().mockResolvedValue(""),
  closeTab: vi.fn().mockResolvedValue(undefined),
}));

import { DiffPane } from "./DiffPane";
import { useDiffStore } from "@/stores/diff-store";
import type { GitFileStatus, WorkspaceSnapshot } from "@/tauri/types";

const TAB = "right-panel:ws-1:diff";
const WORKSPACE = {
  workspace_id: "ws-1",
  cwd: "/repo",
  worktree_path: null,
  tabs: [],
} as unknown as WorkspaceSnapshot;

function status(path: string, additions: number, deletions = 0): GitFileStatus {
  return {
    path,
    status: "modified",
    is_staged: false,
    is_unstaged: true,
    additions,
    deletions,
    conflict_type: null,
  };
}

function diffOf(path: string, added: string): string {
  return [
    `diff --git a/${path} b/${path}`,
    `--- a/${path}`,
    `+++ b/${path}`,
    "@@ -1,2 +1,2 @@",
    " const keep = 0;",
    "-const old = 1;",
    `+${added}`,
  ].join("\n");
}

function renderPane(props: Partial<Parameters<typeof DiffPane>[0]> = {}) {
  return render(<DiffPane tabId={TAB} workspace={WORKSPACE} embedded {...props} />);
}

beforeEach(() => {
  useDiffStore.setState({ tabs: {} });
  useDiffStore.getState().initTab(TAB, { file: "src/a.ts", staged: false });
  mockGetGitStatus.mockReset().mockResolvedValue([status("src/a.ts", 2, 1), status("src/lib/b.ts", 1)]);
  mockGetGitDiff
    .mockReset()
    .mockImplementation((_cwd: string, path: string) =>
      Promise.resolve(diffOf(path, `const in_${path.replace(/\W/g, "_")} = 1;`)),
    );
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("the embedded diff pane", () => {
  it("names the file and where it sits in the set", async () => {
    renderPane();
    await screen.findByText("const in_src_a_ts = 1;");
    const header = screen.getByTestId("diff-file-header");
    expect(header).toHaveTextContent("src/a.ts");
    expect(header).toHaveTextContent("+2");
    expect(header).toHaveTextContent("−1");
    await waitFor(() => expect(screen.getByTestId("diff-file-position")).toHaveTextContent("1/2"));
  });

  it("steps through files with Shift+J / Shift+K and goes back on Escape", async () => {
    const onBack = vi.fn();
    renderPane({ onBack });
    await screen.findByText("const in_src_a_ts = 1;");
    await waitFor(() => expect(screen.getByTestId("diff-file-position")).toHaveTextContent("1/2"));
    const pane = screen.getByTestId("diff-pane");

    fireEvent.keyDown(pane, { key: "J", shiftKey: true });
    await screen.findByText("const in_src_lib_b_ts = 1;");
    expect(useDiffStore.getState().tabs[TAB].filePath).toBe("src/lib/b.ts");
    expect(screen.getByTestId("diff-file-position")).toHaveTextContent("2/2");

    fireEvent.keyDown(pane, { key: "K", shiftKey: true });
    await screen.findByText("const in_src_a_ts = 1;");

    fireEvent.keyDown(pane, { key: "Escape" });
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("takes focus when it opens in place of the row that had it", async () => {
    renderPane();
    expect(screen.getByTestId("diff-pane")).toHaveFocus();
  });

  it("wraps long lines when asked", async () => {
    renderPane({ wrap: true });
    const added = await screen.findByText("const in_src_a_ts = 1;");
    expect(added.closest("[class*='whitespace-pre-wrap']")).not.toBeNull();
  });

  it("says a failed read failed, instead of showing an unchanged file", async () => {
    mockGetGitDiff.mockRejectedValueOnce("index.lock exists");
    renderPane();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't read the diff — index.lock exists");
    expect(screen.queryByText("No changes in this file")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText("const in_src_a_ts = 1;");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("re-reads the open file when an edit moves its status row, keeping the view up", async () => {
    vi.useFakeTimers();
    renderPane();
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText("const in_src_a_ts = 1;")).toBeInTheDocument();

    mockGetGitStatus.mockResolvedValue([status("src/a.ts", 3, 1), status("src/lib/b.ts", 1)]);
    mockGetGitDiff.mockImplementation((_cwd: string, path: string) =>
      Promise.resolve(diffOf(path, "const edited_by_agent = 2;")),
    );
    await act(() => vi.advanceTimersByTimeAsync(5000));

    expect(screen.getByText("const edited_by_agent = 2;")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Updated");
    expect(screen.queryByText("Loading diff…")).toBeNull();

    // An unchanged status row reads nothing again.
    const reads = mockGetGitDiff.mock.calls.length;
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(mockGetGitDiff.mock.calls.length).toBe(reads);
    expect(screen.queryByRole("status")).toBeNull();
  });
});
