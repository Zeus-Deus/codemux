import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Only `getAppState` is needed by the resolver; mock it so we can drive
// the direct-fetch fallback path deterministically.
const getAppStateMock = vi.fn();
vi.mock("@/tauri/commands", () => ({
  getAppState: (...args: unknown[]) => getAppStateMock(...args),
}));

import { waitForWorkspaceCwd } from "./wait-for-workspace-cwd";
// The REAL app-store — the resolver races its subscription against the
// mocked direct fetch.
import { useAppStore } from "@/stores/app-store";

function seed(workspaces: Array<{ workspace_id: string; cwd: string }>) {
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: workspaces[0]?.workspace_id ?? "",
      workspaces,
    } as never,
  });
}

describe("waitForWorkspaceCwd", () => {
  beforeEach(() => {
    getAppStateMock.mockReset();
    // Default: the direct fetch keeps missing, so tests that don't
    // exercise the fallback rely purely on the subscription.
    getAppStateMock.mockResolvedValue({
      schema_version: 1,
      active_workspace_id: "",
      workspaces: [],
    });
    useAppStore.setState({ appState: null });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("fast path: resolves synchronously when the workspace is already in the store", async () => {
    seed([{ workspace_id: "w1", cwd: "/wt/one" }]);
    const cwd = await waitForWorkspaceCwd("w1");
    expect(cwd).toBe("/wt/one");
    // No fetch needed — the store already had it.
    expect(getAppStateMock).not.toHaveBeenCalled();
  });

  it("subscription path: resolves when the app-state event lands the workspace after a delay", async () => {
    seed([]);
    const promise = waitForWorkspaceCwd("w2");
    // Mimic the async `app-state-changed` event arriving late.
    setTimeout(() => seed([{ workspace_id: "w2", cwd: "/wt/two" }]), 15);
    expect(await promise).toBe("/wt/two");
  });

  it("direct-fetch fallback: resolves from a polled get_app_state when the event never fires", async () => {
    seed([]);
    // The event never updates the store; only the polled fetch sees it.
    getAppStateMock.mockResolvedValue({
      schema_version: 1,
      active_workspace_id: "w3",
      workspaces: [{ workspace_id: "w3", cwd: "/wt/three" }],
    });
    const cwd = await waitForWorkspaceCwd("w3");
    expect(cwd).toBe("/wt/three");
    expect(getAppStateMock).toHaveBeenCalled();
    // The fetched snapshot was hydrated into the store for the rest of
    // the UI to catch up.
    expect(
      useAppStore
        .getState()
        .appState?.workspaces.find((w) => w.workspace_id === "w3")?.cwd,
    ).toBe("/wt/three");
  });

  it("timeout: resolves null when the workspace never appears", async () => {
    vi.useFakeTimers();
    seed([]);
    const promise = waitForWorkspaceCwd("missing", 1_000);
    await vi.advanceTimersByTimeAsync(1_200);
    expect(await promise).toBeNull();
  });
});
