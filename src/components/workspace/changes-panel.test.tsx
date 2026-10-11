/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockGetGitStatus = vi.fn();
const mockStage = vi.fn();
const mockUnstage = vi.fn();
const mockDiscard = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getGitStatus: (...a: unknown[]) => mockGetGitStatus(...a),
  getGitBranchInfo: vi.fn().mockResolvedValue(null),
  getMergeState: vi.fn().mockResolvedValue(null),
  checkClaudeAvailable: vi.fn().mockResolvedValue(false),
  gitStageFiles: (...a: unknown[]) => mockStage(...a),
  gitUnstageFiles: (...a: unknown[]) => mockUnstage(...a),
  gitDiscardFile: (...a: unknown[]) => mockDiscard(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

import { ChangesPanel } from "./changes-panel";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { GitFileStatus, WorkspaceSnapshot } from "@/tauri/types";

const WORKSPACE = {
  workspace_id: "ws-1",
  cwd: "/repo",
  worktree_path: null,
  project_root: "/repo",
  is_git: true,
  tabs: [],
} as unknown as WorkspaceSnapshot;

function file(path: string, staged: boolean): GitFileStatus {
  return {
    path,
    status: "modified",
    is_staged: staged,
    is_unstaged: !staged,
    additions: 1,
    deletions: 0,
    conflict_type: null,
  };
}

const FILES = [
  file("src/staged.ts", true),
  file("src/one.ts", false),
  file("src/two.ts", false),
];

function renderPanel(onOpenDiff = vi.fn(), returnFocusPath: string | null = null) {
  const client = new QueryClient();
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <ChangesPanel
          workspace={WORKSPACE}
          onOpenDiff={onOpenDiff}
          returnFocusPath={returnFocusPath}
        />
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return onOpenDiff;
}

const rows = () =>
  Array.from(document.querySelectorAll<HTMLElement>("[data-file-row]"));

beforeEach(() => {
  mockGetGitStatus.mockReset().mockResolvedValue(FILES);
  mockStage.mockReset().mockResolvedValue(undefined);
  mockUnstage.mockReset().mockResolvedValue(undefined);
  mockDiscard.mockReset().mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("before the first status read", () => {
  it("shows it is loading instead of claiming a clean tree", async () => {
    let resolve: (files: GitFileStatus[]) => void = () => {};
    mockGetGitStatus.mockReturnValue(new Promise((r) => (resolve = r)));
    renderPanel();

    expect(screen.getByTestId("changes-loading")).toBeInTheDocument();
    expect(screen.queryByText("Working tree clean")).toBeNull();
    expect(screen.getByRole("button", { name: /Checking changes/ })).toBeDisabled();

    await act(async () => resolve([]));
    expect(await screen.findByText("Working tree clean")).toBeInTheDocument();
    expect(screen.queryByTestId("changes-loading")).toBeNull();
  });
});

describe("file rows from the keyboard", () => {
  it("moves between rows with the arrow keys and opens one with Enter", async () => {
    const onOpenDiff = renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));
    rows()[0].focus();

    fireEvent.keyDown(rows()[0], { key: "ArrowDown" });
    expect(rows()[1]).toHaveFocus();
    fireEvent.keyDown(rows()[1], { key: "ArrowUp" });
    expect(rows()[0]).toHaveFocus();

    fireEvent.keyDown(rows()[0], { key: "Enter" });
    expect(onOpenDiff).toHaveBeenCalledWith("src/staged.ts", true);
  });

  it("stages with S and hands focus to the next file", async () => {
    renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));
    const [, one, two] = rows();
    one.focus();

    fireEvent.keyDown(one, { key: "s" });
    expect(mockStage).toHaveBeenCalledWith("/repo", ["src/one.ts"]);
    expect(two).toHaveFocus();
  });

  it("stages with Shift+S or Caps Lock too", async () => {
    renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));
    const one = rows()[1];
    one.focus();

    fireEvent.keyDown(one, { key: "S", shiftKey: true });
    expect(mockStage).toHaveBeenCalledWith("/repo", ["src/one.ts"]);
  });

  it("gives focus back to the row a diff was opened from", async () => {
    renderPanel(vi.fn(), "src/two.ts");
    await waitFor(() => expect(rows()).toHaveLength(3));
    await waitFor(() => expect(rows()[2]).toHaveFocus());
  });

  it("does not take focus from something the user picked", async () => {
    const elsewhere = document.createElement("button");
    document.body.appendChild(elsewhere);
    elsewhere.focus();
    renderPanel(vi.fn(), "src/two.ts");
    await waitFor(() => expect(rows()).toHaveLength(3));
    expect(elsewhere).toHaveFocus();
    elsewhere.remove();
  });

  it("discards only on a second Delete", async () => {
    renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));
    const one = rows()[1];
    one.focus();

    fireEvent.keyDown(one, { key: "Delete" });
    expect(mockDiscard).not.toHaveBeenCalled();
    expect(one).toHaveAttribute("data-confirm-discard", "true");

    fireEvent.keyDown(one, { key: "Delete" });
    await waitFor(() => expect(mockDiscard).toHaveBeenCalledWith("/repo", "src/one.ts"));
  });

  it("leaves Enter on a row's own buttons to those buttons", async () => {
    const onOpenDiff = renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));
    const stage = rows()[1].querySelector<HTMLElement>("[aria-label='Stage']")!;

    fireEvent.keyDown(stage, { key: "Enter" });
    expect(onOpenDiff).not.toHaveBeenCalled();
  });
});

describe("section actions", () => {
  it("stages every changed file, and unstages every staged one", async () => {
    renderPanel();
    await waitFor(() => expect(rows()).toHaveLength(3));

    fireEvent.click(screen.getByRole("button", { name: "Stage all" }));
    await waitFor(() =>
      expect(mockStage).toHaveBeenCalledWith("/repo", ["src/one.ts", "src/two.ts"]),
    );

    fireEvent.click(screen.getByRole("button", { name: "Unstage all" }));
    await waitFor(() => expect(mockUnstage).toHaveBeenCalledWith("/repo", ["src/staged.ts"]));
  });
});
