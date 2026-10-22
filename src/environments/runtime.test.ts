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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("startEnvironmentRuntime", () => {
  it("shares the initial catalog request with overlapping refreshes and timer ticks", async () => {
    vi.useFakeTimers();
    const catalog = deferred<RegisteredDevice[]>();
    const listDevices = vi.fn(() => catalog.promise);
    const rt = startEnvironmentRuntime({
      registry: { listDevices, connectDevice: vi.fn() },
      dialer: { dial: vi.fn() },
      selfId: "laptop",
      now: () => NOW,
    });
    try {
      const first = rt.refresh();
      const second = rt.refresh();
      await vi.advanceTimersByTimeAsync(120_000);
      expect(listDevices).toHaveBeenCalledTimes(1);
      expect(first).toBe(second);

      catalog.resolve([device("ai-node", 1)]);
      await first;
      expect(opened).toHaveLength(1);
      expect(listDevices).toHaveBeenCalledTimes(1);

      listDevices.mockResolvedValueOnce([]);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(listDevices).toHaveBeenCalledTimes(2);
      expect(opened[0].closed).toBe(true);
      expect(useEnvironmentsStore.getState().environments).toEqual({});
    } finally {
      rt.stop();
    }
  });

  it("allows the next timer tick to retry after a catalog request fails", async () => {
    vi.useFakeTimers();
    const catalog = deferred<RegisteredDevice[]>();
    const listDevices = vi.fn(() => catalog.promise);
    const rt = startEnvironmentRuntime({
      registry: { listDevices, connectDevice: vi.fn() },
      dialer: { dial: vi.fn() },
      selfId: "laptop",
      now: () => NOW,
    });
    try {
      const pending = rt.refresh();
      catalog.reject(new DeviceRegistryError("network", "offline"));
      await pending;

      listDevices.mockResolvedValueOnce([device("ai-node", 1)]);
      await vi.advanceTimersByTimeAsync(60_000);
      expect(listDevices).toHaveBeenCalledTimes(2);
      expect(opened).toHaveLength(1);
    } finally {
      rt.stop();
    }
  });

  it("does not fetch again after stopping and cancels its timer", async () => {
    vi.useFakeTimers();
    const { rt, listDevices } = runtime([[]]);
    await vi.advanceTimersByTimeAsync(0);
    rt.stop();

    await rt.refresh();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(listDevices).toHaveBeenCalledTimes(1);
  });

  it.each(["success", "unauthorized", "network"] as const)(
    "ignores a late %s reply after stopping while a new runtime starts",
    async (reply) => {
      const catalog = deferred<RegisteredDevice[]>();
      const rt = startEnvironmentRuntime({
        registry: { listDevices: () => catalog.promise, connectDevice: vi.fn() },
        dialer: { dial: vi.fn() },
        selfId: "laptop",
        now: () => NOW,
      });
      const pending = rt.refresh();
      rt.stop();
      const replacement = runtime([[device("new-session", 1)]]).rt;
      try {
        await vi.waitFor(() => expect(opened).toHaveLength(1));
        if (reply === "success") catalog.resolve([device("old-session", 1)]);
        else catalog.reject(new DeviceRegistryError(reply, "old session"));
        await pending;

        expect(opened).toEqual([
          { deviceId: "new-session", nodeId: "node-new-session", closed: false },
        ]);
        expect(Object.keys(useEnvironmentsStore.getState().environments)).toEqual(["new-session"]);
      } finally {
        rt.stop();
        replacement.stop();
      }
    },
  );

  it("does not reset a replacement runtime when old cleanup runs again", async () => {
    const old = runtime([[device("old-session", 1)]]).rt;
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    old.stop();
    const replacement = runtime([[device("new-session", 1)]]).rt;
    try {
      await vi.waitFor(() => expect(opened).toHaveLength(2));
      const self = { id: "laptop", name: "laptop", platform: "linux", app_version: "0.25.0", sharing: true };
      useEnvironmentsStore.getState().setSelf(self);
      old.stop();

      expect(opened.map((entry) => entry.closed)).toEqual([true, false]);
      expect(useEnvironmentsStore.getState().self).toEqual(self);
      expect(Object.keys(useEnvironmentsStore.getState().environments)).toEqual(["new-session"]);
    } finally {
      replacement.stop();
    }
  });

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
