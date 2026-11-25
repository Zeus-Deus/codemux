import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { toast } from "@/lib/toast";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { onWorktreeIncludesApplied } from "@/tauri/events";

import { useWorktreeIncludeToast } from "./use-worktree-include-toast";

vi.mock("@/lib/toast", () => ({ toast: { info: vi.fn(), warning: vi.fn() } }));
vi.mock("@/tauri/commands", () => ({ dbGetUiState: vi.fn(), dbSetUiState: vi.fn() }));
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
      expect.objectContaining({ description: expect.stringContaining("config/local.env") }),
    );
    expect(toast.info).not.toHaveBeenCalled();
  });

  it("keeps the default-copy hint for older events", async () => {
    await listener()({ workspace_id: "workspace-1", source: "defaults", copied: [".env"] });

    expect(toast.warning).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledWith(expect.stringContaining("Copied default files (.env)"), expect.anything());
    expect(dbSetUiState).toHaveBeenCalledWith("worktree-include-hint-shown:/fixture/project", "true");
  });
});
