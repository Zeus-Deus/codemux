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

  it("reads the diff once on open, not again when the first status lands", async () => {
    vi.useFakeTimers();
    renderPane();
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText("const in_src_a_ts = 1;")).toBeInTheDocument();
    expect(mockGetGitDiff).toHaveBeenCalledTimes(1);
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
    expect(screen.queryByTestId("diff-loading-bar")).toBeNull();

    // An unchanged re-read changes nothing on screen, and the note clears.
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(screen.getByText("const edited_by_agent = 2;")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("picks up an edit that leaves the +/− counts where they were", async () => {
    vi.useFakeTimers();
    renderPane();
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText("const in_src_a_ts = 1;")).toBeInTheDocument();

    // `+x = 1` becoming `+x = 2`: the status row does not move at all.
    mockGetGitDiff.mockImplementation((_cwd: string, path: string) =>
      Promise.resolve(diffOf(path, "const in_src_a_ts = 2;")),
    );
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(screen.getByText("const in_src_a_ts = 2;")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Updated");
  });

  it("does not carry one file's read error over to the next file", async () => {
    mockGetGitDiff.mockRejectedValueOnce("index.lock exists");
    renderPane();
    await screen.findByRole("alert");
    await waitFor(() => expect(screen.getByTestId("diff-file-position")).toHaveTextContent("1/2"));

    fireEvent.keyDown(screen.getByTestId("diff-pane"), { key: "J", shiftKey: true });
    expect(screen.queryByRole("alert")).toBeNull();
    await screen.findByText("const in_src_lib_b_ts = 1;");
  });

  it("reads Caps Lock j as the next change, not the next file", async () => {
    renderPane();
    await screen.findByText("const in_src_a_ts = 1;");
    await waitFor(() => expect(screen.getByTestId("diff-file-position")).toHaveTextContent("1/2"));

    fireEvent.keyDown(screen.getByTestId("diff-pane"), { key: "J", shiftKey: false });
    expect(useDiffStore.getState().tabs[TAB].filePath).toBe("src/a.ts");
  });

  it("does not call switching to the staged side of the same file an update", async () => {
    renderPane();
    await screen.findByText("const in_src_a_ts = 1;");
    mockGetGitDiff.mockImplementation((_cwd: string, path: string, staged: boolean) =>
      Promise.resolve(diffOf(path, staged ? "const staged_side = 1;" : "const other = 1;")),
    );
    act(() => useDiffStore.getState().setFile(TAB, "src/a.ts", true));
    await screen.findByText("const staged_side = 1;");
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("lets a read slower than the poll land instead of restarting it", async () => {
    vi.useFakeTimers();
    let resolveRead: (raw: string) => void = () => {};
    mockGetGitDiff.mockImplementation(
      () => new Promise<string>((resolve) => (resolveRead = resolve)),
    );
    renderPane();
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText("Loading diff…")).toBeInTheDocument();

    // Two poll ticks pass while the first read is still out.
    await act(() => vi.advanceTimersByTimeAsync(10000));
    expect(mockGetGitDiff).toHaveBeenCalledTimes(1);

    await act(async () => resolveRead(diffOf("src/a.ts", "const slow_read = 1;")));
    expect(screen.getByText("const slow_read = 1;")).toBeInTheDocument();
  });

  it("goes back on Escape before a file is picked", () => {
    useDiffStore.setState({ tabs: {} });
    useDiffStore.getState().initTab(TAB);
    const onBack = vi.fn();
    renderPane({ onBack });
    const pane = screen.getByTestId("diff-pane");
    expect(pane).toHaveTextContent("Select a file to view changes");
    expect(pane).toHaveFocus();
    fireEvent.keyDown(pane, { key: "Escape" });
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("hides a zero count in the header", async () => {
    mockGetGitStatus.mockResolvedValue([status("src/a.ts", 2, 0)]);
    renderPane();
    await screen.findByText("const in_src_a_ts = 1;");
    await waitFor(() => expect(screen.getByTestId("diff-file-header")).toHaveTextContent("+2"));
    expect(screen.getByTestId("diff-file-header")).not.toHaveTextContent("−0");
  });
});
