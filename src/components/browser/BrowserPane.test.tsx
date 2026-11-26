/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  startBrowserStream: vi.fn(),
  browserStreamBridgeEndpoints: vi.fn(),
  agentBrowserRun: vi.fn(),
  agentBrowserPendingConfirmation: vi.fn(),
  agentBrowserRespondConfirmation: vi.fn(),
  copyToClipboard: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  startBrowserStream: (...a: unknown[]) => mocks.startBrowserStream(...a),
  browserStreamBridgeEndpoints: (...a: unknown[]) => mocks.browserStreamBridgeEndpoints(...a),
  agentBrowserRun: (...a: unknown[]) => mocks.agentBrowserRun(...a),
  agentBrowserPendingConfirmation: (...a: unknown[]) => mocks.agentBrowserPendingConfirmation(...a),
  agentBrowserRespondConfirmation: (...a: unknown[]) => mocks.agentBrowserRespondConfirmation(...a),
  activatePane: vi.fn(),
  writeToPty: vi.fn(),
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

function renderPane() {
  return render(<BrowserPane browserId="browser-1" focused={false} visible />);
}

beforeEach(() => {
  mocks.startBrowserStream.mockReset();
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
    mocks.agentBrowserPendingConfirmation.mockResolvedValue({
      id: "c_expire",
      description: "navigation: expired.example",
      expires_at_ms: Date.now() + 1_000,
    });
    renderPane();
    await act(async () => {});
    await act(async () => { vi.advanceTimersByTime(1_200); });
    expect(screen.getByText("Browser confirmation expired. Retry to request it again.")).toBeInTheDocument();
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
