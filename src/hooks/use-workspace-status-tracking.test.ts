import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { needsYouWorkspaceIds } from "@/lib/needs-you";
import { useAppStore } from "@/stores/app-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import type { PaneStatus } from "@/tauri/types";
import { useWorkspaceStatusTracking } from "./use-workspace-status-tracking";

type AppState = NonNullable<ReturnType<typeof useAppStore.getState>["appState"]>;

const ws = (id: string) => ({
  workspace_id: id,
  surfaces: [{ surface_id: `s-${id}`, root: { kind: "terminal", pane_id: `p-${id}` } }],
});

// B is listed before A, so store order alone would put B first.
const workspaces = [ws("b"), ws("a")];

function setStatuses(pane_statuses: Record<string, PaneStatus>) {
  act(() => {
    useAppStore.setState({
      appState: { workspaces, pane_statuses } as unknown as AppState,
    });
  });
}

function needsYouOrder(): string[] {
  const app = useAppStore.getState().appState!;
  return needsYouWorkspaceIds(
    app.workspaces,
    app.pane_statuses,
    useSidebarDensityStore.getState().statusSince,
  );
}

// No sidebar row is mounted here, as with the sidebar collapsed to its rail
// or a full-screen page open: only the app-level tracker sees transitions.
describe("useWorkspaceStatusTracking", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    useSidebarDensityStore.setState({ statusSince: {}, settledAt: {} });
    useAppStore.setState({ appState: null });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("ranks the workspace that blocked first ahead, with no row mounted", () => {
    const { unmount } = renderHook(() => useWorkspaceStatusTracking());
    setStatuses({ "p-a": "permission", "p-b": "working" });
    vi.setSystemTime(2_000);
    setStatuses({ "p-a": "permission", "p-b": "permission" });

    expect(needsYouOrder()).toEqual(["a", "b"]);
    unmount();
  });

  it("restamps a workspace that unblocks and blocks again", () => {
    const { unmount } = renderHook(() => useWorkspaceStatusTracking());
    setStatuses({ "p-a": "permission", "p-b": "permission" });
    vi.setSystemTime(2_000);
    setStatuses({ "p-a": "working", "p-b": "permission" });
    vi.setSystemTime(3_000);
    setStatuses({ "p-a": "permission", "p-b": "permission" });

    // A's first block is gone; it has now waited less than B.
    expect(useSidebarDensityStore.getState().statusSince.a).toEqual({
      status: "permission",
      at: 3_000,
    });
    expect(needsYouOrder()).toEqual(["b", "a"]);
    unmount();
  });

  it("stops observing once unmounted", () => {
    const { unmount } = renderHook(() => useWorkspaceStatusTracking());
    unmount();
    setStatuses({ "p-a": "permission", "p-b": "permission" });
    expect(useSidebarDensityStore.getState().statusSince).toEqual({});
  });
});
