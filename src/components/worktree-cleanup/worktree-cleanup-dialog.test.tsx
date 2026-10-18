/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { StaleWorktree } from "@/tauri/commands";

const { mockScan, mockRemove, mockToast } = vi.hoisted(() => ({
  mockScan: vi.fn(),
  mockRemove: vi.fn(),
  mockToast: { success: vi.fn(), warning: vi.fn(), failure: vi.fn() },
}));

vi.mock("@/tauri/commands", () => ({
  worktreeCleanupScan: (...args: unknown[]) => mockScan(...args),
  worktreeCleanupRemove: (...args: unknown[]) => mockRemove(...args),
}));

vi.mock("@/lib/toast", () => ({
  toast: mockToast,
  errorMessage: (err: unknown) => String(err),
}));

import { WorktreeCleanupDialog } from "./worktree-cleanup-dialog";
import {
  listTitles,
  staleReasonLabel,
  useWorktreeCleanupStore,
} from "./worktree-cleanup-store";

function worktree(overrides: Partial<StaleWorktree> = {}): StaleWorktree {
  return {
    path: "/wt/merged",
    title: "merged",
    branch: "feat/merged",
    project: "codemux",
    source: "archived",
    reason: { kind: "branch_merged", base: "main" },
    kept_because: null,
    ...overrides,
  };
}

const merged = worktree();
const prMerged = worktree({
  path: "/wt/pr",
  title: "pr",
  reason: { kind: "pr_merged", number: 7 },
});
const dirty = worktree({
  path: "/wt/dirty",
  title: "dirty",
  source: "closed",
  reason: { kind: "idle", days: 40 },
  kept_because: "2 uncommitted or untracked files",
});

beforeEach(() => {
  mockScan.mockReset();
  mockRemove.mockReset();
  for (const fn of Object.values(mockToast)) fn.mockReset();
  useWorktreeCleanupStore.setState({ open: true });
});
afterEach(cleanup);

describe("WorktreeCleanupDialog", () => {
  it("lists removable worktrees with reasons and kept ones without a checkbox", async () => {
    mockScan.mockResolvedValue([merged, prMerged, dirty]);
    render(<WorktreeCleanupDialog />);

    expect(await screen.findByText("Merged into main")).toBeInTheDocument();
    expect(screen.getByText("PR #7 merged")).toBeInTheDocument();
    expect(screen.getByText("2 uncommitted or untracked files")).toBeInTheDocument();
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Remove 2 worktrees" })).toBeEnabled();
  });

  it("removes only the checked worktrees and closes", async () => {
    mockScan.mockResolvedValue([merged, prMerged, dirty]);
    mockRemove.mockResolvedValue({ removed: [merged], kept: [] });
    render(<WorktreeCleanupDialog />);

    await userEvent.click(await screen.findByRole("checkbox", { name: "Remove pr" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove 1 worktree" }));

    expect(mockRemove).toHaveBeenCalledWith(["/wt/merged"]);
    await waitFor(() => expect(useWorktreeCleanupStore.getState().open).toBe(false));
    expect(mockToast.success).toHaveBeenCalledWith("Removed 1 worktree", expect.anything());
  });

  it("reports worktrees the backend kept at removal time", async () => {
    mockScan.mockResolvedValue([merged]);
    mockRemove.mockResolvedValue({
      removed: [],
      kept: [{ ...merged, kept_because: "1 commit isn't on any remote" }],
    });
    render(<WorktreeCleanupDialog />);

    await userEvent.click(await screen.findByRole("button", { name: "Remove 1 worktree" }));

    await waitFor(() =>
      expect(mockToast.warning).toHaveBeenCalledWith("Kept 1 worktree", {
        description: "merged: 1 commit isn't on any remote",
      }),
    );
    expect(mockToast.success).not.toHaveBeenCalled();
  });

  it("explains when nothing is stale", async () => {
    mockScan.mockResolvedValue([]);
    render(<WorktreeCleanupDialog />);

    expect(await screen.findByText(/No stale worktrees/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Remove/ })).toBeNull();
  });
});

describe("labels", () => {
  it("names the idle source", () => {
    expect(staleReasonLabel(dirty)).toBe("Closed 40 days ago");
    expect(staleReasonLabel({ ...dirty, source: "archived" })).toBe("Archived 40 days ago");
  });

  it("shortens long title lists", () => {
    expect(listTitles([merged])).toBe("merged");
    expect(listTitles([merged, prMerged])).toBe("merged and pr");
    expect(listTitles([merged, prMerged, dirty])).toBe("merged, pr and 1 more");
  });
});
