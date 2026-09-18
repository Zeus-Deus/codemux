import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadRemoteWorkspace, remoteClientId, saveRemoteWorkspace, setRemoteViewHost } from "./client-view";

beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });
afterEach(() => { vi.unstubAllGlobals(); });

describe("remote client view persistence", () => {
  it("keeps selections separate for hosts sharing the relay origin", () => {
    setRemoteViewHost("host-one");
    saveRemoteWorkspace("workspace-one");
    setRemoteViewHost("host-two");
    expect(loadRemoteWorkspace()).toBeNull();
    saveRemoteWorkspace("workspace-two");
    setRemoteViewHost("host-one");
    expect(loadRemoteWorkspace()).toBe("workspace-one");
  });

  it("tolerates unavailable storage and keeps a stable client id for reconnects", () => {
    // jsdom Storage treats instance assignments as stored keys; replace the
    // global so this also works with the Node 26 fallback used by test setup.
    const getItem = vi.fn(() => { throw new Error("blocked"); });
    const setItem = vi.fn(() => { throw new Error("blocked"); });
    vi.stubGlobal("localStorage", { getItem, setItem });
    expect(() => saveRemoteWorkspace("workspace")).not.toThrow();
    expect(loadRemoteWorkspace()).toBeNull();
    expect(setItem).toHaveBeenCalledOnce();
    expect(getItem).toHaveBeenCalledOnce();
    expect(remoteClientId()).toBe(remoteClientId());
    expect(remoteClientId()).toMatch(/^[a-zA-Z0-9-]+$/);
  });
});
