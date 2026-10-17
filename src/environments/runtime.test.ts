import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const opened: Array<{ deviceId: string; nodeId: string; closed: boolean }> = [];
vi.mock("./connection", () => ({
  openEnvironmentConnection: ({ device }: { device: { deviceId: string; nodeId: string } }) => {
    const entry = { deviceId: device.deviceId, nodeId: device.nodeId, closed: false };
    opened.push(entry);
    return { close: () => (entry.closed = true) };
  },
}));

import { DeviceRegistryError, type RegisteredDevice } from "@/remote/device-registry";

import { startEnvironmentRuntime } from "./runtime";
import { useEnvironmentsStore } from "./store";

const NOW = Date.parse("2026-10-15T10:00:00Z");

function device(deviceId: string, minutesAgo: number | null, nodeId = `node-${deviceId}`): RegisteredDevice {
  return {
    id: deviceId,
    deviceId,
    nodeId,
    name: deviceId,
    platform: "linux",
    lastSeenAt: minutesAgo === null ? null : new Date(NOW - minutesAgo * 60_000).toISOString(),
  };
}

beforeEach(() => {
  opened.length = 0;
  useEnvironmentsStore.getState().reset();
});
afterEach(() => vi.useRealTimers());

function runtime(lists: Array<RegisteredDevice[] | Error>) {
  const listDevices = vi.fn(async () => {
    const next = lists.length > 1 ? lists.shift()! : lists[0];
    if (next instanceof Error) throw next;
    return next;
  });
  const rt = startEnvironmentRuntime({
    registry: { listDevices, connectDevice: vi.fn() },
    dialer: { dial: vi.fn() },
    selfId: "laptop",
    now: () => NOW,
  });
  return { rt, listDevices };
}

describe("startEnvironmentRuntime", () => {
  it("connects to recently seen machines other than itself and lists stale ones offline", async () => {
    const { rt } = runtime([[device("laptop", 1), device("ai-node", 2), device("old-box", 60)]]);
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    expect(opened[0]).toMatchObject({ deviceId: "ai-node", closed: false });

    const { environments } = useEnvironmentsStore.getState();
    expect(Object.keys(environments).sort()).toEqual(["ai-node", "old-box"]);
    expect(environments["old-box"].status).toBe("offline");
    expect(environments["ai-node"].status).toBe("connecting");
    rt.stop();
    expect(opened[0].closed).toBe(true);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
  });

  it("keeps a connection across refreshes and replaces it when the machine moves", async () => {
    const { rt } = runtime([
      [device("ai-node", 1)],
      [device("ai-node", 1)],
      [device("ai-node", 1, "node-new")],
      [],
    ]);
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    await rt.refresh();
    expect(opened).toHaveLength(1);

    await rt.refresh();
    expect(opened.map((o) => [o.nodeId, o.closed])).toEqual([
      ["node-ai-node", true],
      ["node-new", false],
    ]);

    await rt.refresh();
    expect(opened[1].closed).toBe(true);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
    rt.stop();
  });

  it("drops every machine when the account session ends, but keeps them on a network blip", async () => {
    const { rt } = runtime([
      [device("ai-node", 1)],
      new DeviceRegistryError("network", "offline"),
      new DeviceRegistryError("unauthorized", "Sign in again.", 401),
    ]);
    await vi.waitFor(() => expect(opened).toHaveLength(1));

    await rt.refresh();
    expect(opened[0].closed).toBe(false);
    expect(Object.keys(useEnvironmentsStore.getState().environments)).toEqual(["ai-node"]);

    await rt.refresh();
    expect(opened[0].closed).toBe(true);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
    rt.stop();
  });
});

describe("useEnvironmentsStore.update", () => {
  it("ignores a stale snapshot from the same backend but accepts a restarted one", () => {
    const store = useEnvironmentsStore.getState();
    store.setCatalog([device("ai-node", 1)]);
    const snap = (instance: string, revision: number) =>
      ({ snapshot_instance: instance, snapshot_revision: revision, workspaces: [] }) as never;

    store.update("ai-node", { appState: snap("a", 5) });
    store.update("ai-node", { appState: snap("a", 4), status: "connected" });
    let view = useEnvironmentsStore.getState().environments["ai-node"];
    expect(view.appState).toMatchObject({ snapshot_revision: 5 });
    expect(view.status).toBe("connected");

    store.update("ai-node", { appState: snap("b", 1) });
    view = useEnvironmentsStore.getState().environments["ai-node"];
    expect(view.appState).toMatchObject({ snapshot_instance: "b", snapshot_revision: 1 });
  });
});
