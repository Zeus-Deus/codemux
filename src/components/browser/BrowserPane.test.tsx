/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  startBrowserStream: vi.fn(),
  ensureBrowserPresetNavigation: vi.fn(),
  browserPresetNavigationStatus: vi.fn(),
  browserStreamBridgeEndpoints: vi.fn(),
  agentBrowserRun: vi.fn(),
  agentBrowserPendingConfirmation: vi.fn(),
  agentBrowserRespondConfirmation: vi.fn(),
  copyToClipboard: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  isRemoteClient: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  startBrowserStream: (...a: unknown[]) => mocks.startBrowserStream(...a),
  ensureBrowserPresetNavigation: (...a: unknown[]) => mocks.ensureBrowserPresetNavigation(...a),
  browserPresetNavigationStatus: (...a: unknown[]) => mocks.browserPresetNavigationStatus(...a),
  browserStreamBridgeEndpoints: (...a: unknown[]) => mocks.browserStreamBridgeEndpoints(...a),
  agentBrowserRun: (...a: unknown[]) => mocks.agentBrowserRun(...a),
  agentBrowserPendingConfirmation: (...a: unknown[]) => mocks.agentBrowserPendingConfirmation(...a),
  agentBrowserRespondConfirmation: (...a: unknown[]) => mocks.agentBrowserRespondConfirmation(...a),
  activatePane: vi.fn(),
  writeToPty: vi.fn(),
}));

vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => mocks.isRemoteClient(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

vi.mock("@/lib/clipboard", () => ({
  COPY_FAILED_MESSAGE: "copy failed",
  copyToClipboard: (...a: unknown[]) => mocks.copyToClipboard(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: { success: mocks.toastSuccess, error: mocks.toastError },
}));

import { BrowserPane, MAX_AUTO_START_RETRIES } from "./BrowserPane";

/** Opens on the next tick and records what the pane sends. */
class FakeWebSocket {
  static readonly OPEN = 1;
  static sent: Array<Record<string, unknown>> = [];
  static urls: string[] = [];
  static opened = 0;
  static lastUrl: string | null = null;
  static lastSocket: FakeWebSocket | null = null;
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
    FakeWebSocket.urls.push(url);
    FakeWebSocket.lastUrl = url;
    FakeWebSocket.lastSocket = this;
    setTimeout(() => {
      this.readyState = FakeWebSocket.OPEN;
      FakeWebSocket.opened++;
      this.onopen?.();
    }, 0);
  }
  send(data: string) {
    FakeWebSocket.sent.push(JSON.parse(data));
  }
  close() {
    this.readyState = 3;
  }
}

function renderPane(browserId = "browser-1") {
  return render(<BrowserPane browserId={browserId} focused={false} visible />);
}

function emitFrame() {
  FakeWebSocket.lastSocket?.onmessage?.({
    data: JSON.stringify({ type: "frame", data: "", metadata: { deviceWidth: 1280, deviceHeight: 720 } }),
  });
}

function setPresetBrowser(browserId: string, url = "https://example.com") {
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: "ws-1",
      workspaces: [],
      terminal_sessions: [],
      browser_sessions: [{
        browser_id: browserId,
        title: "Example",
        current_url: url,
        history: [],
        history_index: -1,
        is_loading: false,
        last_error: null,
      }],
      agent_browser_sessions: [],
      notifications: [],
      detected_ports: [],
      pane_statuses: {},
      persistence: {
        schema_version: 1,
        stores_layout_metadata: true,
        stores_terminal_metadata: true,
        stores_live_process_state: false,
      },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

beforeEach(() => {
  mocks.startBrowserStream.mockReset();
  mocks.ensureBrowserPresetNavigation.mockReset().mockResolvedValue("completed");
  mocks.browserPresetNavigationStatus.mockReset().mockResolvedValue(null);
  mocks.browserStreamBridgeEndpoints.mockReset().mockResolvedValue({
    ws_url: "ws://127.0.0.1:9777/private/browser/9999/ws",
    http_base: "http://127.0.0.1:9777/private/browser/9999",
  });
  mocks.agentBrowserRun.mockReset().mockResolvedValue(null);
  mocks.agentBrowserPendingConfirmation.mockReset().mockResolvedValue(null);
  mocks.agentBrowserRespondConfirmation.mockReset().mockResolvedValue(undefined);
  mocks.copyToClipboard.mockReset();
  mocks.toastSuccess.mockReset();
  mocks.toastError.mockReset();
  mocks.isRemoteClient.mockReset().mockReturnValue(false);
  FakeWebSocket.sent = [];
  FakeWebSocket.urls = [];
  FakeWebSocket.opened = 0;
  FakeWebSocket.lastUrl = null;
  FakeWebSocket.lastSocket = null;
  vi.stubGlobal("WebSocket", FakeWebSocket);
  useAppStore.setState({ appState: null });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("BrowserPane start failures", () => {
  it("shows a confirmation raised by an agent while the pane is connected", async () => {
    vi.useFakeTimers();
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");
    renderPane();
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(FakeWebSocket.urls).toHaveLength(1);
    mocks.agentBrowserPendingConfirmation.mockResolvedValue({
      id: "c_agent",
      description: "navigation: agent.example",
      expires_at_ms: Date.now() + 60_000,
    });

    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(screen.getByText("navigation: agent.example")).toBeInTheDocument();
    expect(FakeWebSocket.lastSocket?.readyState).toBe(3);
    expect(FakeWebSocket.urls).toHaveLength(1);
  });

  it("keeps the bridge closed until a pending browser action is approved", async () => {
    mocks.startBrowserStream
      .mockRejectedValueOnce("Browser confirmation is pending for session browser-1")
      .mockResolvedValue("ws://127.0.0.1:9223");
    mocks.agentBrowserPendingConfirmation.mockResolvedValue({
      id: "c_123",
      description: "navigation: example.com",
      expires_at_ms: Date.now() + 60_000,
    });

    renderPane();
    await act(async () => {});
    expect(screen.getByText("navigation: example.com")).toBeInTheDocument();
    expect(FakeWebSocket.urls).toHaveLength(0);
    expect(mocks.browserStreamBridgeEndpoints).not.toHaveBeenCalled();

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    });
    await vi.waitFor(() => expect(FakeWebSocket.urls).toHaveLength(1));
    expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith("browser-1", "c_123", true);
    expect(mocks.browserStreamBridgeEndpoints).toHaveBeenCalledTimes(1);
  });

  it("shows the next approval without reopening the stream after a confirmed action", async () => {
    mocks.startBrowserStream
      .mockRejectedValueOnce("Browser confirmation is pending for session browser-1")
      .mockResolvedValue("ws://127.0.0.1:9223");
    mocks.agentBrowserPendingConfirmation.mockResolvedValue({
      id: "c_first",
      description: "Approve navigation",
      expires_at_ms: Date.now() + 60_000,
    });
    mocks.agentBrowserRespondConfirmation
      .mockResolvedValueOnce({
        id: "c_second",
        description: "Approve plugin action",
        expires_at_ms: Date.now() + 60_000,
      })
      .mockResolvedValueOnce(null);

    renderPane();
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(screen.getByText("Approve plugin action")).toBeInTheDocument());
    expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith("browser-1", "c_first", true);
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
    expect(mocks.browserStreamBridgeEndpoints).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(mocks.startBrowserStream).toHaveBeenCalledTimes(2));
    expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith("browser-1", "c_second", true);
  });

  it("reads the backend's cached preset result after approval reconnects or remounts the pane", async () => {
    setPresetBrowser("browser-1");
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");
    const firstPrompt = {
      id: "host_first",
      description: "Approve navigation",
      expires_at_ms: Date.now() + 60_000,
    };
    let backendPending: typeof firstPrompt | null = null;
    let submitted = 0;
    mocks.agentBrowserPendingConfirmation.mockImplementation(async () => backendPending);
    mocks.ensureBrowserPresetNavigation.mockImplementation(async () => {
      if (submitted === 0) {
        submitted++;
        backendPending = firstPrompt;
        return "pending";
      }
      return backendPending ? "pending" : "completed";
    });
    mocks.browserPresetNavigationStatus.mockImplementation(async () => backendPending ? "pending" : "completed");
    mocks.agentBrowserRespondConfirmation.mockImplementation(async () => {
      backendPending = null;
      return null;
    });
    const pane = renderPane();
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    expect(FakeWebSocket.opened).toBe(1);
    act(emitFrame);
    await vi.waitFor(() => expect(submitted).toBe(1));
    await vi.waitFor(() => expect(screen.getByText("Approve navigation")).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2));
    act(emitFrame);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 350)); });
    expect(submitted).toBe(1);
    expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith("browser-1", "host_first", true);

    pane.unmount();
    renderPane();
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(3));
    act(emitFrame);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 350)); });
    expect(submitted).toBe(1);
    expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledWith("browser-1", "https://example.com", false);
  });

  it("keeps a failed preset visible through later frames and retries only on request", async () => {
    const browserId = "browser-preset-failure";
    setPresetBrowser(browserId);
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");
    mocks.ensureBrowserPresetNavigation.mockResolvedValueOnce("failed").mockResolvedValue("completed");

    renderPane(browserId);
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    act(emitFrame);
    await vi.waitFor(() => expect(screen.getByText("Preset navigation did not complete. Check the page, then retry if needed.")).toBeInTheDocument());
    act(emitFrame);
    expect(screen.getByText("Preset navigation did not complete. Check the page, then retry if needed.")).toBeInTheDocument();
    expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Retry now" }));
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2));
    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(2));
    expect(mocks.ensureBrowserPresetNavigation).toHaveBeenLastCalledWith(browserId, "https://example.com", true);
  });

  it("waits for host approval on a remote pane without reading host-only prompt details", async () => {
    const browserId = "browser-remote-preset";
    setPresetBrowser(browserId);
    mocks.isRemoteClient.mockReturnValue(true);
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");
    mocks.ensureBrowserPresetNavigation.mockResolvedValueOnce("pending").mockResolvedValue("completed");
    let presetStatus = "pending";
    mocks.browserPresetNavigationStatus.mockImplementation(async () => presetStatus);

    renderPane(browserId);
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    act(emitFrame);
    await vi.waitFor(() => expect(screen.getByText("Browser approval is waiting on the host device.")).toBeInTheDocument());
    expect(mocks.agentBrowserPendingConfirmation).not.toHaveBeenCalled();
    expect(mocks.browserStreamBridgeEndpoints).not.toHaveBeenCalled();

    presetStatus = "completed";
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2), { timeout: 3500 });
    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(2));
    expect(mocks.ensureBrowserPresetNavigation).toHaveBeenLastCalledWith(browserId, "https://example.com", false);
    expect(mocks.agentBrowserPendingConfirmation).not.toHaveBeenCalled();
  });

  it("does not apply a former browser's preset poll or stale frame after switching panes", async () => {
    const firstId = "browser-preset-first";
    const secondId = "browser-preset-second";
    setPresetBrowser(firstId, "https://first.example");
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");
    let firstPending = false;
    mocks.ensureBrowserPresetNavigation.mockImplementation(async (sessionId: string) => {
      if (sessionId === firstId) firstPending = true;
      return sessionId === firstId ? "pending" : "completed";
    });
    const firstPrompt = {
      id: "first_prompt",
      description: "Approve first browser",
      expires_at_ms: Date.now() + 60_000,
    };
    mocks.agentBrowserPendingConfirmation.mockImplementation(async (sessionId: string) =>
      sessionId === firstId && firstPending ? firstPrompt : null);
    let releaseFirstStatus: ((state: string) => void) | undefined;
    const firstStatus = new Promise<string>((resolve) => { releaseFirstStatus = resolve; });
    mocks.browserPresetNavigationStatus.mockImplementation((sessionId: string) =>
      sessionId === firstId ? firstStatus : Promise.resolve(null));
    let releaseFirstApproval: ((details: typeof firstPrompt) => void) | undefined;
    const firstApproval = new Promise<typeof firstPrompt>((resolve) => { releaseFirstApproval = resolve; });
    mocks.agentBrowserRespondConfirmation.mockImplementation(() => firstApproval);
    const onStatusChange = vi.fn();

    const pane = render(<BrowserPane browserId={firstId} focused={false} visible onStatusChange={onStatusChange} />);
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    const firstSocket = FakeWebSocket.lastSocket;
    act(emitFrame);
    await vi.waitFor(() => expect(screen.getByText("Approve first browser")).toBeInTheDocument());
    await vi.waitFor(() => expect(mocks.browserPresetNavigationStatus).toHaveBeenCalledWith(firstId));
    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith(firstId, "first_prompt", true));

    act(() => {
      setPresetBrowser(secondId, "https://second.example");
      pane.rerender(<BrowserPane browserId={secondId} focused={false} visible onStatusChange={onStatusChange} />);
    });
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2));
    expect(screen.queryByText("Approve first browser")).not.toBeInTheDocument();
    onStatusChange.mockClear();
    act(() => firstSocket?.onmessage?.({
      data: JSON.stringify({ type: "frame", data: "", metadata: { deviceWidth: 1280, deviceHeight: 720 } }),
    }));
    expect(onStatusChange).not.toHaveBeenCalledWith("live");
    expect(mocks.ensureBrowserPresetNavigation).not.toHaveBeenCalledWith(secondId, "https://second.example", false);

    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledWith(secondId, "https://second.example", false));
    const firstReadsBefore = mocks.agentBrowserPendingConfirmation.mock.calls
      .filter(([sessionId]) => sessionId === firstId).length;
    await act(async () => { releaseFirstStatus?.("pending"); });
    await act(async () => { releaseFirstApproval?.({ ...firstPrompt, id: "old_follow_up", description: "Old follow-up" }); });
    expect(screen.queryByText("Approve first browser")).not.toBeInTheDocument();
    expect(screen.queryByText("Old follow-up")).not.toBeInTheDocument();
    expect(mocks.agentBrowserPendingConfirmation.mock.calls.filter(([sessionId]) => sessionId === firstId)).toHaveLength(firstReadsBefore);
  });

  it("ignores an old pending-prompt poll after approval reveals a follow-up", async () => {
    const firstPrompt = {
      id: "first_prompt",
      description: "Approve navigation",
      expires_at_ms: Date.now() + 60_000,
    };
    const followUp = {
      id: "follow_up",
      description: "Approve plugin",
      expires_at_ms: Date.now() + 60_000,
    };
    mocks.startBrowserStream.mockRejectedValue("Browser confirmation is pending for session browser-1");
    let releaseOldPoll: ((details: typeof firstPrompt) => void) | undefined;
    const oldPoll = new Promise<typeof firstPrompt>((resolve) => { releaseOldPoll = resolve; });
    let holdPoll = false;
    mocks.agentBrowserPendingConfirmation.mockImplementation(() => holdPoll ? oldPoll : Promise.resolve(firstPrompt));
    mocks.agentBrowserRespondConfirmation.mockResolvedValue(followUp);

    renderPane();
    await vi.waitFor(() => expect(screen.getByText("Approve navigation")).toBeInTheDocument());
    const initialReads = mocks.agentBrowserPendingConfirmation.mock.calls.length;
    holdPoll = true;
    await vi.waitFor(() => expect(mocks.agentBrowserPendingConfirmation.mock.calls.length).toBeGreaterThan(initialReads), { timeout: 3500 });

    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(screen.getByText("Approve plugin")).toBeInTheDocument());
    await act(async () => { releaseOldPoll?.(firstPrompt); });
    expect(screen.getByText("Approve plugin")).toBeInTheDocument();
    expect(screen.queryByText("Approve navigation")).not.toBeInTheDocument();
  });

  it("shows a cached failure after a pane remounts during the first preset open", async () => {
    const browserId = "browser-preset-remount";
    setPresetBrowser(browserId);
    let resolveFirst: ((status: string) => void) | undefined;
    const firstResult = new Promise<string>((resolve) => { resolveFirst = resolve; });
    let starts = 0;
    mocks.startBrowserStream.mockImplementation(async () => {
      if (++starts > 1) await firstResult;
      return "ws://127.0.0.1:9223";
    });
    mocks.ensureBrowserPresetNavigation.mockImplementationOnce(async () => firstResult)
      .mockResolvedValue("failed");

    const firstPane = renderPane(browserId);
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(1));
    firstPane.unmount();
    renderPane(browserId);
    await vi.waitFor(() => expect(mocks.startBrowserStream).toHaveBeenCalledTimes(2));
    await act(async () => { resolveFirst?.("failed"); });
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2));
    act(emitFrame);
    await vi.waitFor(() => expect(screen.getByText("Preset navigation did not complete. Check the page, then retry if needed.")).toBeInTheDocument());
    expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(2);
  });

  it("rechecks the backend preset after a live same-port socket reconnect", async () => {
    const browserId = "browser-preset-reconnect";
    setPresetBrowser(browserId);
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9223");

    renderPane(browserId);
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(1));
    act(() => FakeWebSocket.lastSocket?.onclose?.());
    await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(2), { timeout: 2500 });
    act(emitFrame);
    await vi.waitFor(() => expect(mocks.ensureBrowserPresetNavigation).toHaveBeenCalledTimes(2));
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
  });

  it("shows a consumed replay failure without leaving the spent approval visible", async () => {
    mocks.startBrowserStream.mockRejectedValue("Browser confirmation is pending for session browser-1");
    let pending: { id: string; description: string; expires_at_ms: number } | null = {
      id: "host_first",
      description: "Approve navigation",
      expires_at_ms: Date.now() + 60_000,
    };
    mocks.agentBrowserPendingConfirmation.mockImplementation(async () => pending);
    mocks.agentBrowserRespondConfirmation.mockImplementation(async () => {
      pending = null;
      throw new Error("Browser action failed after confirmation: navigation failed");
    });

    renderPane();
    await vi.waitFor(() => expect(screen.getByText("Approve navigation")).toBeInTheDocument());
    fireEvent.click(screen.getByRole("button", { name: "Approve browser action" }));
    await vi.waitFor(() => expect(screen.getByText(/navigation failed/)).toBeInTheDocument());
    expect(screen.queryByText("Approve navigation")).not.toBeInTheDocument();
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
  });

  it("denies a pending action without starting another browser", async () => {
    mocks.startBrowserStream.mockRejectedValue("Browser confirmation is pending for session browser-1");
    mocks.agentBrowserPendingConfirmation.mockResolvedValue({
      id: "c_deny",
      description: "navigation: blocked.example",
      expires_at_ms: Date.now() + 60_000,
    });
    renderPane();
    await act(async () => {});

    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await act(async () => {});
    expect(mocks.agentBrowserRespondConfirmation).toHaveBeenCalledWith("browser-1", "c_deny", false);
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Browser action denied")).toBeInTheDocument();
  });

  it("expires an unanswered confirmation without silently retrying", async () => {
    vi.useFakeTimers();
    mocks.startBrowserStream.mockRejectedValue("Browser confirmation is pending for session browser-1");
    const details = {
      id: "c_expire",
      description: "navigation: expired.example",
      expires_at_ms: Date.now() + 1_000,
    };
    mocks.agentBrowserPendingConfirmation.mockImplementation(async () =>
      Date.now() < details.expires_at_ms ? details : null);
    renderPane();
    await act(async () => {});
    await act(async () => { await vi.advanceTimersByTimeAsync(2_100); });
    expect(screen.getByText("Browser confirmation is no longer pending. Retry to reconnect.")).toBeInTheDocument();
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
    expect(mocks.agentBrowserRespondConfirmation).not.toHaveBeenCalled();
  });

  it("keeps an in-flight prompt visible past its deadline and shows the next approval", async () => {
    vi.useFakeTimers();
    mocks.startBrowserStream.mockRejectedValue("Browser confirmation is pending for session browser-1");
    let backendPending = {
      id: "host_first",
      description: "Approve navigation",
      expires_at_ms: Date.now() + 1_000,
    };
    mocks.agentBrowserPendingConfirmation.mockImplementation(async () => backendPending);

    renderPane();
    await act(async () => {});
    expect(screen.getByText("Approve navigation")).toBeInTheDocument();
    await act(async () => { await vi.advanceTimersByTimeAsync(1_200); });
    expect(screen.getByText("Approve navigation")).toBeInTheDocument();
    backendPending = {
      id: "host_second",
      description: "Approve plugin action",
      expires_at_ms: Date.now() + 60_000,
    };
    await act(async () => { await vi.advanceTimersByTimeAsync(900); });
    expect(screen.getByText("Approve plugin action")).toBeInTheDocument();
    expect(screen.queryByText("Approve navigation")).not.toBeInTheDocument();
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
    expect(mocks.agentBrowserRespondConfirmation).not.toHaveBeenCalled();
  });

  it("connects through the fresh bridge when the session snapshot has an old URL", async () => {
    useAppStore.setState({
      appState: {
        schema_version: 1,
        active_workspace_id: "ws-1",
        workspaces: [],
        terminal_sessions: [],
        browser_sessions: [],
        agent_browser_sessions: [{
          session_id: "abs-1",
          workspace_id: "ws-1",
          cli_session_name: "browser-1",
          stream_url: "ws://localhost:9223",
          current_url: null,
          is_active: true,
          pane_id: null,
          browser_id: "browser-1",
          user_dismissed: false,
        }],
        notifications: [],
        detected_ports: [],
        pane_statuses: {},
        persistence: {
          schema_version: 1,
          stores_layout_metadata: true,
          stores_terminal_metadata: true,
          stores_live_process_state: false,
        },
        config: {} as AppStateSnapshot["config"],
      },
    });
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9224");
    mocks.browserStreamBridgeEndpoints.mockResolvedValue({
      ws_url: "ws://127.0.0.1:9777/private/browser/9224/ws",
      http_base: "http://127.0.0.1:9777/private/browser/9224",
    });

    renderPane();
    await act(async () => {
      await vi.waitFor(() => expect(FakeWebSocket.urls).toHaveLength(1));
    });
    expect(FakeWebSocket.urls).toEqual(["ws://127.0.0.1:9777/private/browser/9224/ws"]);
  });

  it("does not connect directly when the desktop bridge is unavailable", async () => {
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9999");
    mocks.browserStreamBridgeEndpoints.mockRejectedValue("bridge unavailable");
    renderPane();
    await act(async () => {});

    expect(screen.getByText("Failed to start browser: bridge unavailable")).toBeInTheDocument();
    expect(FakeWebSocket.opened).toBe(0);
  });

  it("retries a failed start a few times, then waits for Retry now", async () => {
    vi.useFakeTimers();
    mocks.startBrowserStream.mockRejectedValue("daemon exited");
    renderPane();
    await act(async () => {});

    expect(screen.getByText("Failed to start browser: daemon exited")).toBeInTheDocument();
    expect(screen.getByText("Retrying in 10s")).toBeInTheDocument();
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);

    for (let i = 0; i < MAX_AUTO_START_RETRIES; i++) {
      await act(async () => {
        vi.advanceTimersByTime(10_000);
      });
    }
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(MAX_AUTO_START_RETRIES + 1);
    // Out of automatic attempts: no countdown, no more starts.
    expect(screen.queryByText(/Retrying in/)).toBeNull();
    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(MAX_AUTO_START_RETRIES + 1);

    // Retry now starts again and re-arms the automatic retries.
    fireEvent.click(screen.getByRole("button", { name: "Retry now" }));
    await act(async () => {});
    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(MAX_AUTO_START_RETRIES + 2);
    expect(screen.getByText("Retrying in 10s")).toBeInTheDocument();
  });

  it("copies error details through the clipboard fallback", async () => {
    mocks.startBrowserStream.mockRejectedValue("daemon exited");
    mocks.copyToClipboard.mockResolvedValueOnce(true).mockResolvedValueOnce(false);
    renderPane();
    await act(async () => {});

    const copy = screen.getByRole("button", { name: "Copy details" });
    await act(async () => fireEvent.click(copy));
    expect(mocks.copyToClipboard).toHaveBeenCalledWith(
      expect.stringContaining("Error: Failed to start browser: daemon exited"),
    );
    expect(mocks.toastSuccess).toHaveBeenCalledWith("Copied error details");

    await act(async () => fireEvent.click(copy));
    expect(mocks.toastError).toHaveBeenCalledWith("copy failed");
  });
});

describe("BrowserPane keyboard", () => {
  async function renderLive() {
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9999");
    renderPane();
    // Stream start, then the socket's open on a later tick.
    await act(async () => {
      await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    });
    return screen.getByLabelText("Browser page");
  }

  const keyUps = () => FakeWebSocket.sent.filter((m) => m.eventType === "keyUp");

  it("connects through the authenticated desktop bridge", async () => {
    await renderLive();
    expect(mocks.browserStreamBridgeEndpoints).toHaveBeenCalledWith("browser-1");
    expect(FakeWebSocket.lastUrl).toBe("ws://127.0.0.1:9777/private/browser/9999/ws");
  });

  it("gets a fresh bridge capability after a daemon restart on the same port", async () => {
    vi.useFakeTimers();
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9999");
    mocks.browserStreamBridgeEndpoints
      .mockResolvedValueOnce({
        ws_url: "ws://127.0.0.1:9777/old/ws",
        http_base: "http://127.0.0.1:9777/old",
      })
      .mockResolvedValueOnce({
        ws_url: "ws://127.0.0.1:9777/new/ws",
        http_base: "http://127.0.0.1:9777/new",
      });
    renderPane();
    await act(async () => {});
    expect(FakeWebSocket.lastUrl).toBe("ws://127.0.0.1:9777/old/ws");

    act(() => FakeWebSocket.lastSocket?.onclose?.());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500);
    });

    expect(mocks.startBrowserStream).toHaveBeenCalledTimes(1);
    expect(mocks.browserStreamBridgeEndpoints).toHaveBeenCalledTimes(2);
    expect(FakeWebSocket.lastUrl).toBe("ws://127.0.0.1:9777/new/ws");
  });

  it("keeps browser shortcuts, press and release, away from the page", async () => {
    const canvas = await renderLive();

    fireEvent.keyDown(canvas, { key: "ArrowLeft", code: "ArrowLeft", altKey: true });
    fireEvent.keyUp(canvas, { key: "ArrowLeft", code: "ArrowLeft", altKey: true });
    expect(mocks.agentBrowserRun).toHaveBeenCalledWith("browser-1", "back", {});

    fireEvent.keyDown(canvas, { key: "r", code: "KeyR", ctrlKey: true });
    fireEvent.keyUp(canvas, { key: "r", code: "KeyR", ctrlKey: true });
    expect(mocks.agentBrowserRun).toHaveBeenCalledWith("browser-1", "reload", {});

    expect(FakeWebSocket.sent.filter((m) => m.type === "input_keyboard")).toEqual([]);

    // Ordinary keys still reach the page, release included.
    fireEvent.keyDown(canvas, { key: "a", code: "KeyA" });
    fireEvent.keyUp(canvas, { key: "a", code: "KeyA" });
    expect(keyUps()).toEqual([expect.objectContaining({ key: "a" })]);
  });

  it("focuses the address bar on Ctrl+L", async () => {
    const canvas = await renderLive();
    fireEvent.keyDown(canvas, { key: "l", code: "KeyL", ctrlKey: true });
    fireEvent.keyUp(canvas, { key: "l", code: "KeyL", ctrlKey: true });
    expect(screen.getByLabelText("Address")).toHaveFocus();
    expect(FakeWebSocket.sent.filter((m) => m.type === "input_keyboard")).toEqual([]);
  });
});

describe("BrowserPane viewport presets", () => {
  async function renderLive() {
    mocks.startBrowserStream.mockResolvedValue("ws://127.0.0.1:9999");
    renderPane();
    await act(async () => {
      await vi.waitFor(() => expect(FakeWebSocket.opened).toBe(1));
    });
  }

  async function pickPreset(name: RegExp) {
    await userEvent.click(screen.getByRole("button", { name: "More browser actions" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name }));
  }

  const resizes = () => FakeWebSocket.sent.filter((m) => m.type === "resize");

  it("pins the agent's viewport to a preset and hands it back on Fit pane", async () => {
    // jsdom lays nothing out; give the pane container a real size.
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(900);
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
    await renderLive();
    mocks.agentBrowserRun.mockClear();
    FakeWebSocket.sent = [];

    await pickPreset(/^Mobile/);
    expect(mocks.agentBrowserRun).toHaveBeenCalledWith("browser-1", "viewport", { width: 390, height: 844 });
    expect(resizes()).toEqual([{ type: "resize", width: 390, height: 844 }]);

    await pickPreset(/^Fit pane/);
    expect(mocks.agentBrowserRun).toHaveBeenLastCalledWith("browser-1", "viewport", { width: 900, height: 600 });
    expect(resizes()).toEqual([
      { type: "resize", width: 390, height: 844 },
      { type: "resize", width: 900, height: 600 },
    ]);
  });
});
