import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { renderHook } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  RELOAD_REQUESTED_EVENT,
  cancelInterfaceReload,
  reloadInterfaceNow,
  requestInterfaceReload,
  useInterfaceReloadRequests,
  useInterfaceReloadStore,
} from "./interface-reload";

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

function setRemoteClient(remote: boolean) {
  (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = remote;
}

beforeEach(() => {
  vi.clearAllMocks();
  invokeMock.mockResolvedValue(undefined);
  listenMock.mockResolvedValue(vi.fn());
  useInterfaceReloadStore.setState({ prompting: false });
  setRemoteClient(false);
});

afterEach(() => {
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});

describe("requestInterfaceReload", () => {
  it("asks before reloading anything", () => {
    requestInterfaceReload();

    expect(useInterfaceReloadStore.getState().prompting).toBe(true);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("reloads on a second ask — the escape hatch for a page that cannot paint", () => {
    requestInterfaceReload();
    requestInterfaceReload();

    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("reload_main_window");
    expect(useInterfaceReloadStore.getState().prompting).toBe(false);
  });

  it("closes without reloading when cancelled", () => {
    requestInterfaceReload();
    cancelInterfaceReload();

    expect(useInterfaceReloadStore.getState().prompting).toBe(false);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

describe("reloadInterfaceNow", () => {
  it("asks the app process for a webview reload and nothing else", () => {
    reloadInterfaceNow();

    // Exactly one command, and it is the webview reload — never a process
    // restart, a backend shutdown or a session teardown.
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("reload_main_window");
  });

  it("survives a failing command without throwing", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    invokeMock.mockRejectedValueOnce(new Error("no main window"));

    expect(() => reloadInterfaceNow()).not.toThrow();
    await vi.waitFor(() => expect(consoleError).toHaveBeenCalled());
    consoleError.mockRestore();
  });

  it("reloads its own tab in the web remote client, never the desktop webview", () => {
    setRemoteClient(true);
    const reload = vi.fn();
    Object.defineProperty(window, "location", {
      configurable: true,
      value: { ...window.location, reload },
    });

    reloadInterfaceNow();

    expect(reload).toHaveBeenCalledTimes(1);
    expect(invokeMock).not.toHaveBeenCalled();
  });
});

describe("useInterfaceReloadRequests", () => {
  it("answers the app process and opens the dialog", async () => {
    renderHook(() => useInterfaceReloadRequests());

    expect(listenMock).toHaveBeenCalledWith(
      RELOAD_REQUESTED_EVENT,
      expect.any(Function),
    );
    const handler = listenMock.mock.calls[0][1] as (event: {
      payload: { token: number };
    }) => void;
    handler({ payload: { token: 7 } });

    // The answer has to go out before the app process's grace period ends,
    // otherwise it reloads the page without asking the user.
    expect(invokeMock).toHaveBeenCalledWith("ack_reload_request", { token: 7 });
    expect(invokeMock).not.toHaveBeenCalledWith("reload_main_window");
    expect(useInterfaceReloadStore.getState().prompting).toBe(true);
  });

  it("stays out of the web remote client", () => {
    setRemoteClient(true);
    renderHook(() => useInterfaceReloadRequests());

    expect(listenMock).not.toHaveBeenCalled();
  });

  it("unsubscribes on unmount so repeat mounts cannot stack listeners", async () => {
    const unlisten = vi.fn();
    listenMock.mockResolvedValue(unlisten);
    const { unmount } = renderHook(() => useInterfaceReloadRequests());
    await vi.waitFor(() => expect(listenMock).toHaveBeenCalled());

    unmount();

    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });
});
