/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

vi.mock("@/tauri/commands", () => ({
  renameWorkspace: (...args: unknown[]) => mocks.renameWorkspace(...args),
  undockBrowserFromRightPanel: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({ toast: mocks.toast }));

import {
  RenameWorkspaceDialog,
  WORKSPACE_NAME_MAX_LENGTH,
  getWorkspaceRenameStatus,
} from "./rename-workspace-dialog";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";

function makeWorkspace(
  workspaceId: string,
  title: string,
  branch: string | null = "master",
): WorkspaceSnapshot {
  return {
    workspace_id: workspaceId,
    title,
    workspace_type: "standard",
    cwd: "/home/test/world-bench",
    project_root: "/home/test/world-bench",
    git_branch: branch,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
  } as WorkspaceSnapshot;
}

function openDialog() {
  const workspaces = [
    makeWorkspace("ws-1", "add-gpt5-benchmark-runs"),
    makeWorkspace("ws-2", "review-open-prs", "main"),
  ];
  useAppStore.setState({
    appState: {
      workspaces,
      active_workspace_id: "ws-1",
    } as unknown as AppStateSnapshot,
  });
  useUIStore.setState({ renameWorkspaceId: "ws-1" });
  render(<RenameWorkspaceDialog />);
}

beforeEach(() => {
  mocks.renameWorkspace.mockReset().mockResolvedValue(undefined);
  mocks.toast.success.mockReset();
  mocks.toast.error.mockReset();
  useAppStore.setState({ appState: null });
  useUIStore.setState({ renameWorkspaceId: null });
});

afterEach(() => cleanup());

describe("getWorkspaceRenameStatus", () => {
  it("matches the design validation rules", () => {
    const base = {
      originalName: "original",
      branch: "master",
      takenNames: ["already-used"],
    } as const;

    expect(getWorkspaceRenameStatus({ ...base, name: "  " }).kind).toBe(
      "empty",
    );
    expect(
      getWorkspaceRenameStatus({ ...base, name: "x".repeat(49) }).kind,
    ).toBe("long");
    expect(
      getWorkspaceRenameStatus({ ...base, name: "already-used" }).kind,
    ).toBe("taken");
    expect(getWorkspaceRenameStatus({ ...base, name: "original" }).kind).toBe(
      "idle",
    );
    expect(getWorkspaceRenameStatus({ ...base, name: "new-name" }).kind).toBe(
      "ready",
    );
  });
});

describe("RenameWorkspaceDialog", () => {
  it("opens with the name selected and shows project plus branch context", () => {
    openDialog();

    const input = screen.getByRole("textbox", { name: "Workspace name" });
    expect(input).toHaveValue("add-gpt5-benchmark-runs");
    expect(input).toHaveFocus();
    expect(screen.getByText("world-bench · master")).toBeInTheDocument();
    expect(screen.getByText(`23/${WORKSPACE_NAME_MAX_LENGTH}`)).toBeInTheDocument();
  });

  it("shows inline empty and duplicate-name errors without calling the backend", async () => {
    const user = userEvent.setup();
    openDialog();
    const input = screen.getByRole("textbox", { name: "Workspace name" });

    await user.clear(input);
    expect(screen.getByText("Name can’t be empty")).toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(mocks.renameWorkspace).not.toHaveBeenCalled();

    await user.type(input, "review-open-prs");
    expect(
      screen.getByText("Another workspace already uses this name"),
    ).toBeInTheDocument();
    await user.keyboard("{Enter}");
    expect(mocks.renameWorkspace).not.toHaveBeenCalled();
  });

  it("trims, submits, closes, and confirms the new label", async () => {
    const user = userEvent.setup();
    openDialog();
    const input = screen.getByRole("textbox", { name: "Workspace name" });

    await user.clear(input);
    await user.type(input, "  benchmark-cleanup  ");
    expect(
      screen.getByText(
        "Branch stays master — only the label changes",
      ),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Rename" }));

    await waitFor(() => {
      expect(mocks.renameWorkspace).toHaveBeenCalledWith(
        "ws-1",
        "benchmark-cleanup",
      );
    });
    expect(useUIStore.getState().renameWorkspaceId).toBeNull();
    expect(mocks.toast.success).toHaveBeenCalledWith(
      "Renamed to benchmark-cleanup",
    );
  });

  it("closes without renaming when Cancel is pressed", async () => {
    const user = userEvent.setup();
    openDialog();

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(useUIStore.getState().renameWorkspaceId).toBeNull();
    expect(mocks.renameWorkspace).not.toHaveBeenCalled();
  });
});
