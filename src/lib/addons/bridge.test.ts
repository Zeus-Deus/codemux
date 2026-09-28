import { beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class<T> {
    onmessage: (value: T) => void = () => {};
  },
}));
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => false,
}));
import { resubscribeAddons, subscribeAddons } from "./bridge";
import { useAddonsStore } from "@/stores/addons-store";
import type { AddonEvent } from "./types";

beforeEach(() => {
  invoke.mockReset();
  useAddonsStore.setState({ ready: false, contextRevision: 0 });
});

it("reopens a failed event stream for the same receiver and binds the workspace again", async () => {
  const receive = vi.fn();
  invoke.mockRejectedValueOnce({
    message: "The add-on registry at /data/addons-v1 could not be opened",
    data: { code: "STORAGE_UNAVAILABLE" },
  });
  await expect(subscribeAddons(receive)).rejects.toBeTruthy();
  invoke.mockResolvedValue(null);
  await resubscribeAddons(() => "workspace-2");
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "addon_subscribe",
    "addon_subscribe",
    "addon_context_changed",
  ]);
  expect(invoke).toHaveBeenLastCalledWith("addon_context_changed", {
    workspaceId: "workspace-2",
  });
  const channel = invoke.mock.calls[1][1].channel as {
    onmessage: (event: AddonEvent) => void;
  };
  channel.onmessage({ type: "inventory" });
  expect(receive).toHaveBeenCalledWith({ type: "inventory" });
  expect(useAddonsStore.getState().ready).toBe(true);
});

it("leaves a working event stream alone", async () => {
  invoke.mockResolvedValue(null);
  await subscribeAddons(vi.fn());
  invoke.mockClear();
  await resubscribeAddons(() => "workspace-1");
  expect(invoke).not.toHaveBeenCalled();
});
