import { describe, it, expect, vi, beforeEach } from "vitest";

// The command wrappers are thin `invoke` calls; mock the Tauri core so we can
// observe the exact command name + args each wrapper sends over the IPC/WS.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  Channel: class {},
}));

import { invoke } from "@tauri-apps/api/core";
import {
  activateWorkspace,
  archiveWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
} from "./commands";
import {
  adoptLocalFallbackActivation,
  resetLocalWorkspaceActivations,
  wasActivatedLocally,
} from "@/lib/local-activation";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockClear();
  resetLocalWorkspaceActivations();
});

describe("activateWorkspace", () => {
  // Every navigation surface bottoms out here, so recording the activation in
  // the wrapper is what keeps the local-activation record complete — callers
  // that use the raw command (Open folder, Clone, the new-workspace dialog,
  // onboarding) would otherwise land on a workspace nothing ever fills.
  it("records the activation as local and invokes activate_workspace", async () => {
    expect(wasActivatedLocally("ws-1")).toBe(false);

    await activateWorkspace("ws-1");

    expect(mockInvoke).toHaveBeenCalledWith("activate_workspace", {
      workspaceId: "ws-1",
    });
    expect(wasActivatedLocally("ws-1")).toBe(true);
  });

  it("does not record workspaces this client never activated", async () => {
    await activateWorkspace("ws-1");
    expect(wasActivatedLocally("ws-2")).toBe(false);
  });
});

describe("workspace removal commands", () => {
  // Removing the active workspace lets the BACKEND choose the next one, so
  // there is no activation to record — only this marker, which keeps the
  // workspace we land on counted as ours to fill.
  it.each([
    ["closeWorkspace", () => closeWorkspace("ws-1", false)],
    [
      "closeWorkspaceWithWorktree",
      () => closeWorkspaceWithWorktree("ws-1", true, false, false),
    ],
    ["archiveWorkspace", () => archiveWorkspace("ws-1")],
  ])("%s marks the backend's fallback activation as local", async (_, run) => {
    await run();

    expect(adoptLocalFallbackActivation("ws-fallback")).toBe(true);
    expect(wasActivatedLocally("ws-fallback")).toBe(true);
  });

  it("does not adopt an activation when nothing was closed", () => {
    expect(adoptLocalFallbackActivation("ws-theirs")).toBe(false);
    expect(wasActivatedLocally("ws-theirs")).toBe(false);
  });
});
