import { beforeEach, describe, expect, it, vi } from "vitest";

const activateWorkspaceInteraction = vi.fn();
const activatePane = vi.fn();
const { toastInfo, toastError } = vi.hoisted(() => ({
  toastInfo: vi.fn(),
  toastError: vi.fn(),
}));
let workspaces: { workspace_id: string }[] = [];

vi.mock("@/lib/perf/instrumented-activate", () => ({
  activateWorkspaceInteraction: (id: string) => activateWorkspaceInteraction(id),
}));
vi.mock("@/tauri/commands", () => ({
  activatePane: (id: string) => activatePane(id),
}));
vi.mock("@/lib/toast", () => ({
  toast: { info: toastInfo, error: toastError },
}));
vi.mock("@/stores/app-store", () => ({
  useAppStore: { getState: () => ({ appState: { workspaces } }) },
}));

import { openNotificationTarget } from "./open-notification-target";
import { useMobileNavigationStore } from "@/stores/mobile-navigation-store";

describe("openNotificationTarget", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    activateWorkspaceInteraction.mockResolvedValue(undefined);
    activatePane.mockResolvedValue(undefined);
    workspaces = [{ workspace_id: "ws-1" }];
    useMobileNavigationStore.setState({ home: true });
  });

  it("opens the workspace, then the agent's pane", async () => {
    expect(
      await openNotificationTarget({ workspace_id: "ws-1", pane_id: "pane-7" }),
    ).toBe(true);

    expect(activateWorkspaceInteraction).toHaveBeenCalledWith("ws-1");
    expect(activatePane).toHaveBeenCalledWith("pane-7");
    expect(activateWorkspaceInteraction.mock.invocationCallOrder[0]).toBeLessThan(
      activatePane.mock.invocationCallOrder[0],
    );
    expect(useMobileNavigationStore.getState().home).toBe(false);
  });

  it("says so instead of switching when the workspace has closed", async () => {
    workspaces = [];
    expect(
      await openNotificationTarget({ workspace_id: "ws-1", pane_id: "pane-7" }),
    ).toBe(false);

    expect(activateWorkspaceInteraction).not.toHaveBeenCalled();
    expect(toastInfo).toHaveBeenCalledWith("This workspace is no longer open.");
  });

  it("reports a failed switch", async () => {
    activateWorkspaceInteraction.mockRejectedValue(new Error("boom"));
    expect(
      await openNotificationTarget({ workspace_id: "ws-1", pane_id: "pane-7" }),
    ).toBe(false);

    expect(activatePane).not.toHaveBeenCalled();
    expect(toastError).toHaveBeenCalledWith("Could not open the agent", {
      description: "Error: boom",
    });
  });
});
