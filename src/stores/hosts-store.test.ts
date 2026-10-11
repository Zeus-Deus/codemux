import { beforeEach, describe, expect, it, vi } from "vitest";
import type { HostView } from "@/tauri/commands";

const listeners = new Map<string, () => void>();
const listen = vi.fn(async (event: string, cb: () => void) => {
  listeners.set(event, cb);
  return () => listeners.delete(event);
});
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: () => void) => listen(event, cb),
}));

const hostsList = vi.fn<() => Promise<HostView[]>>();
vi.mock("@/tauri/commands", () => ({
  hostsList: () => hostsList(),
}));

import {
  HOSTS_CHANGED_EVENT,
  __resetHostsStoreForTests,
  useHostsStore,
} from "./hosts-store";

function host(id: number, name: string): HostView {
  return {
    id,
    server_id: `srv-${id}`,
    name,
    ssh_target: `u@${name}`,
    created_at: "",
    updated_at: "",
    dirty: false,
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("hosts store", () => {
  beforeEach(() => {
    __resetHostsStoreForTests();
    listeners.clear();
    listen.mockClear();
    hostsList.mockReset();
  });

  it("reloads when an account sync renames or removes devices", async () => {
    hostsList.mockResolvedValueOnce([host(1, "box"), host(2, "old")]);
    await useHostsStore.getState().init();
    await useHostsStore.getState().init();
    expect(listen).toHaveBeenCalledTimes(1);

    hostsList.mockResolvedValueOnce([host(1, "renamed")]);
    listeners.get(HOSTS_CHANGED_EVENT)?.();
    await flush();

    expect(useHostsStore.getState().hosts).toEqual([host(1, "renamed")]);
  });

  it("reloads again when the change lands during a load", async () => {
    let finishFirst: (list: HostView[]) => void = () => {};
    hostsList.mockReturnValueOnce(
      new Promise((resolve) => {
        finishFirst = resolve;
      }),
    );
    const first = useHostsStore.getState().init();
    await flush();

    // That load may have read the list before the sync wrote it.
    hostsList.mockResolvedValueOnce([host(3, "pulled")]);
    listeners.get(HOSTS_CHANGED_EVENT)?.();
    finishFirst([]);
    await first;
    await flush();

    expect(hostsList).toHaveBeenCalledTimes(2);
    expect(useHostsStore.getState().hosts).toEqual([host(3, "pulled")]);
  });
});
