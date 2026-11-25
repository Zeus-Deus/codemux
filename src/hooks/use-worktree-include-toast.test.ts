import { cleanup, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { toast } from "@/lib/toast";
import { copyMissingNestedWorktreeIncludes, dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { onWorktreeIncludesApplied } from "@/tauri/events";

import { useWorktreeIncludeToast } from "./use-worktree-include-toast";

vi.mock("@/lib/toast", () => ({ toast: { info: vi.fn(), warning: vi.fn(), success: vi.fn() } }));
vi.mock("@/tauri/commands", () => ({
  copyMissingNestedWorktreeIncludes: vi.fn(),
  dbGetUiState: vi.fn(),
  dbSetUiState: vi.fn(),
}));
vi.mock("@/tauri/events", () => ({ onWorktreeIncludesApplied: vi.fn() }));
const remote = vi.hoisted(() => ({ value: false }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => remote.value }));
vi.mock("@/stores/app-store", () => ({
  useAppStore: {
    getState: () => ({
      appState: { workspaces: [{ workspace_id: "workspace-1", project_root: "/fixture/project" }] },
    }),
  },
}));

function listener() {
  renderHook(() => useWorktreeIncludeToast());
  return vi.mocked(onWorktreeIncludesApplied).mock.calls[0][0];
}

beforeEach(() => {
  remote.value = false;
  vi.mocked(onWorktreeIncludesApplied).mockResolvedValue(vi.fn());
  vi.mocked(dbGetUiState).mockResolvedValue(null);
  vi.mocked(dbSetUiState).mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("useWorktreeIncludeToast", () => {
  it("reports skipped nested paths even when no files were copied", async () => {
    await listener()({
      workspace_id: "workspace-1",
      source: "file",
      copied: [],
      skipped_nested: ["config/local.env"],
    });

    expect(toast.warning).toHaveBeenCalledWith(
      "Nested worktree files skipped",
      expect.objectContaining({ description: "Skipped config/local.env." }),
    );
    expect(toast.info).not.toHaveBeenCalled();
  });

  it("reports errors and skipped paths separately and offers desktop recovery", async () => {
    vi.mocked(copyMissingNestedWorktreeIncludes).mockResolvedValue({
      source: "file",
      copied: ["config/local.env"],
      skipped_nested: [],
      errors: [],
    });
    await listener()({
      workspace_id: "workspace-1",
      source: "file",
      copied: [],
      errors: [".env: permission denied"],
      skipped_nested: ["config/local.env"],
    });

    expect(toast.warning).toHaveBeenCalledTimes(2);
    expect(toast.warning).toHaveBeenCalledWith(
      "Some worktree includes could not be copied",
      expect.objectContaining({ description: ".env: permission denied" }),
    );
    const warning = vi.mocked(toast.warning).mock.calls.find(([title]) => title === "Nested worktree files skipped");
    expect(warning?.[1]?.description).toBe("Skipped config/local.env.");
    const action = warning?.[1]?.action;
    if (!action || typeof action !== "object" || !("onClick" in action)) {
      throw new Error("Expected a desktop recovery action");
    }
    render(createElement("button", { onClick: action.onClick }, action.label));
    await userEvent.click(screen.getByRole("button", { name: "Copy missing nested files" }));

    expect(copyMissingNestedWorktreeIncludes).toHaveBeenCalledExactlyOnceWith("workspace-1");
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith(
      "Copied missing nested worktree files",
      expect.objectContaining({ description: "config/local.env" }),
    ));
  });

  it("points remote clients to the desktop without offering recovery in the browser", async () => {
    remote.value = true;
    await listener()({
      workspace_id: "workspace-1",
      source: "file",
      copied: [],
      skipped_nested: ["config/local.env"],
    });

    expect(toast.warning).toHaveBeenCalledWith(
      "Nested worktree files skipped",
      expect.objectContaining({
        action: undefined,
        description: "Skipped config/local.env. Copy missing nested files from the desktop.",
      }),
    );
  });

  it("keeps the default-copy hint for older events", async () => {
    await listener()({ workspace_id: "workspace-1", source: "defaults", copied: [".env"] });

    expect(toast.warning).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledWith(expect.stringContaining("Copied default files (.env)"), expect.anything());
    expect(dbSetUiState).toHaveBeenCalledWith("worktree-include-hint-shown:/fixture/project", "true");
  });
});
