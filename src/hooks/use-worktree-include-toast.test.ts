import { cleanup, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { onWorktreeIncludesApplied } from "@/tauri/events";

import { useWorktreeIncludeToast } from "./use-worktree-include-toast";

vi.mock("@/lib/clipboard", () => ({
  copyToClipboard: vi.fn(),
  COPY_FAILED_MESSAGE: "Couldn't copy — select and copy manually.",
}));
vi.mock("@/lib/toast", () => ({ toast: { info: vi.fn(), warning: vi.fn(), success: vi.fn(), error: vi.fn() } }));
vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn(),
  dbSetUiState: vi.fn(),
}));
vi.mock("@/tauri/events", () => ({ onWorktreeIncludesApplied: vi.fn() }));
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
  vi.mocked(copyToClipboard).mockResolvedValue(true);
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
      expect.objectContaining({
        description: "Skipped config/local.env. On the worktree host, copy missing files from the main checkout.",
      }),
    );
    expect(toast.info).not.toHaveBeenCalled();
  });

  it("reports errors and skipped paths separately and copies the complete path list", async () => {
    const skipped = ["config/local.env", "config/db.env", "test/local.env", "test/path\nwith-newline.env"];
    await listener()({
      workspace_id: "workspace-1",
      source: "file",
      copied: [],
      errors: [".env: permission denied"],
      skipped_nested: skipped,
    });

    expect(toast.warning).toHaveBeenCalledTimes(2);
    expect(toast.warning).toHaveBeenCalledWith(
      "Some worktree includes could not be copied",
      expect.objectContaining({ description: ".env: permission denied" }),
    );
    const warning = vi.mocked(toast.warning).mock.calls.find(([title]) => title === "Nested worktree files skipped");
    expect(warning?.[1]?.description).toContain("config/local.env, config/db.env, test/local.env and 1 more");
    const action = warning?.[1]?.action;
    if (!action || typeof action !== "object" || !("onClick" in action)) {
      throw new Error("Expected a clipboard action");
    }
    render(createElement("button", { onClick: action.onClick }, action.label));
    await userEvent.click(screen.getByRole("button", { name: "Copy skipped paths" }));

    expect(copyToClipboard).toHaveBeenCalledExactlyOnceWith(JSON.stringify(skipped, null, 2));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Copied skipped paths"));
  });

  it("keeps the default-copy hint for older events", async () => {
    await listener()({ workspace_id: "workspace-1", source: "defaults", copied: [".env"] });

    expect(toast.warning).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledWith(expect.stringContaining("Copied default files (.env)"), expect.anything());
    expect(dbSetUiState).toHaveBeenCalledWith("worktree-include-hint-shown:/fixture/project", "true");
  });
});
