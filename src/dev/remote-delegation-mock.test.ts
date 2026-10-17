import { afterEach, beforeAll, expect, it } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { useDelegationTasks } from "@/stores/delegation-store";

type MockWindow = Window & {
  __CODEMUX_REMOTE__?: boolean;
  __TAURI_INTERNALS__: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };
};
const mockWindow = window as unknown as MockWindow;

beforeAll(async () => { await import("./tauri-mock"); });
afterEach(() => { delete mockWindow.__CODEMUX_REMOTE__; });

it("provides empty task and grant lists with no remote delegations", async () => {
  expect(await mockWindow.__TAURI_INTERNALS__.invoke("delegation_list", { paneId: "mock-pane" })).toEqual([]);
  expect(await mockWindow.__TAURI_INTERNALS__.invoke("delegation_grants", { paneId: "mock-pane" })).toEqual([]);
});

it("hydrates the actual task hook without showing a false remote-task error", async () => {
  const { result, unmount } = renderHook(() => useDelegationTasks("mock-pane", "mock-thread"));
  try {
    await waitFor(() => {
      expect(result.current.subscriptionHealth).toBe("connected");
      expect(result.current.loading).toBe(false);
    });
    expect(result.current.error).toBeNull();
    expect(result.current.tasks).toEqual([]);
  } finally {
    unmount();
  }
});

it("continues to refuse remote-client delegation before dispatch", async () => {
  mockWindow.__CODEMUX_REMOTE__ = true;
  await expect(mockWindow.__TAURI_INTERNALS__.invoke("delegation_list", { paneId: "mock-pane" })).rejects.toContain("REMOTE_UNSUPPORTED");
});
