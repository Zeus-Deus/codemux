import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RegisteredDevice } from "@/remote/device-registry";
import type { EnvironmentConnectionOptions } from "./connection";
import type { EnvironmentDescriptor } from "./store";

const { invokeMock, listDevices, opened } = vi.hoisted(() => ({
  invokeMock: vi.fn<() => Promise<EnvironmentDescriptor | null>>(),
  listDevices: vi.fn<() => Promise<RegisteredDevice[]>>(),
  opened: [] as Array<{ deviceId: string; closed: boolean }>,
}));

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  invoke: invokeMock,
}));
vi.mock("@/remote/device-registry", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/remote/device-registry")>(),
  DeviceRegistry: class {
    listDevices = listDevices;
    connectDevice = vi.fn();
  },
}));
vi.mock("./native", () => ({
  NativeIrohDialer: class {},
  nativeAccountFetch: vi.fn(),
}));
vi.mock("./connection", () => ({
  openEnvironmentConnection: ({ device }: EnvironmentConnectionOptions) => {
    const entry = { deviceId: device.deviceId, closed: false };
    opened.push(entry);
    return { close: () => { entry.closed = true; } };
  },
}));

import { createSeedAppState } from "@/dev/mock-fixtures";
import { useAuthStore } from "@/stores/auth-store";
import { useEnvironmentsStore } from "./store";
import { useEnvironmentRuntime } from "./use-environment-runtime";

const self: EnvironmentDescriptor = {
  id: "self", name: "laptop", platform: "linux", app_version: "0.24.0", sharing: true,
};
const machine: RegisteredDevice = {
  id: "account-a-device", deviceId: "account-a-env", nodeId: "account-a-node",
  name: "account-a-desktop", platform: "linux", lastSeenAt: new Date().toISOString(),
};

function signIn(id: string) {
  useAuthStore.getState().setUser({ id, email: `${id}@example.test`, name: id, image: null });
}

function useRuntime() {
  useEnvironmentRuntime(useAuthStore((state) => state.isAuthenticated));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.clearAllMocks();
  opened.length = 0;
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
  useEnvironmentsStore.getState().reset();
  useAuthStore.setState({ user: null, isAuthenticated: false });
  invokeMock.mockResolvedValue(self);
  listDevices.mockResolvedValue([machine]);
});
afterEach(() => {
  cleanup();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
});

describe("useEnvironmentRuntime account identity", () => {
  it("closes old connections and clears snapshots before the replacement catalog resolves", async () => {
    signIn("account-a");
    renderHook(useRuntime);
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    useEnvironmentsStore.getState().update(machine.deviceId, { appState: createSeedAppState() });
    expect(useEnvironmentsStore.getState().environments[machine.deviceId].appState).not.toBeNull();

    const catalog = deferred<RegisteredDevice[]>();
    let catalogStart: { closed: boolean; ids: string[] } | null = null;
    listDevices.mockImplementationOnce(() => {
      catalogStart = {
        closed: opened[0].closed,
        ids: Object.keys(useEnvironmentsStore.getState().environments),
      };
      return catalog.promise;
    });
    act(() => signIn("account-b"));
    expect(useAuthStore.getState().isAuthenticated).toBe(true);
    expect(opened[0].closed).toBe(true);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
    await vi.waitFor(() => expect(listDevices).toHaveBeenCalledTimes(2));
    expect(catalogStart).toEqual({ closed: true, ids: [] });
    expect(useEnvironmentsStore.getState().environments).toEqual({});

    const replacement = { ...machine, id: "account-b-device", deviceId: "account-b-env" };
    await act(async () => catalog.resolve([replacement]));
    expect(opened).toEqual([
      { deviceId: machine.deviceId, closed: true },
      { deviceId: replacement.deviceId, closed: false },
    ]);
    expect(Object.keys(useEnvironmentsStore.getState().environments)).toEqual([replacement.deviceId]);
  });

  it("does not retain the previous account's data when the replacement catalog fails", async () => {
    signIn("account-a");
    renderHook(useRuntime);
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    useEnvironmentsStore.getState().update(machine.deviceId, { appState: createSeedAppState() });
    listDevices.mockRejectedValueOnce(new Error("network unavailable"));

    await act(async () => signIn("account-b"));
    expect(listDevices).toHaveBeenCalledTimes(2);
    expect(opened[0].closed).toBe(true);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
  });

  it("ignores an old catalog that resolves after the account has changed", async () => {
    const oldCatalog = deferred<RegisteredDevice[]>();
    listDevices.mockReturnValueOnce(oldCatalog.promise).mockResolvedValueOnce([]);
    signIn("account-a");
    renderHook(useRuntime);
    await vi.waitFor(() => expect(listDevices).toHaveBeenCalledOnce());

    await act(async () => signIn("account-b"));
    expect(listDevices).toHaveBeenCalledTimes(2);
    await act(async () => oldCatalog.resolve([machine]));
    expect(opened).toHaveLength(0);
    expect(useEnvironmentsStore.getState().environments).toEqual({});
  });

  it("cancels old initialization and resets self before the replacement identity loads", async () => {
    const oldInfo = deferred<EnvironmentDescriptor | null>();
    const newInfo = deferred<EnvironmentDescriptor | null>();
    invokeMock.mockReturnValueOnce(oldInfo.promise).mockReturnValueOnce(newInfo.promise);
    signIn("account-a");
    renderHook(useRuntime);
    act(() => signIn("account-b"));
    expect(useEnvironmentsStore.getState().self).toBeNull();
    await act(async () => oldInfo.resolve(self));
    expect(listDevices).not.toHaveBeenCalled();
    expect(useEnvironmentsStore.getState().self).toBeNull();
    await act(async () => newInfo.resolve(self));
    expect(listDevices).toHaveBeenCalledOnce();
    expect(opened).toHaveLength(1);
  });

  it("keeps connections on same-account updates and tears them down on sign-out", async () => {
    signIn("account-a");
    renderHook(useRuntime);
    await vi.waitFor(() => expect(opened).toHaveLength(1));
    await act(async () => signIn("account-a"));
    expect(listDevices).toHaveBeenCalledOnce();
    expect(opened[0].closed).toBe(false);

    act(() => useAuthStore.getState().setUser(null));
    expect(opened[0].closed).toBe(true);
    expect(useEnvironmentsStore.getState()).toMatchObject({ self: null, environments: {} });
  });
});
