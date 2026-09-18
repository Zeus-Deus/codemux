import { beforeEach, describe, expect, it, vi } from "vitest";
import { loadRemoteWorkspace, remoteClientId, saveRemoteWorkspace, setRemoteViewHost } from "./client-view";

beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

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
    vi.spyOn(localStorage, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(localStorage, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(() => saveRemoteWorkspace("workspace")).not.toThrow();
    expect(loadRemoteWorkspace()).toBeNull();
    expect(remoteClientId()).toBe(remoteClientId());
    expect(remoteClientId()).toMatch(/^[a-zA-Z0-9-]+$/);
  });
});
