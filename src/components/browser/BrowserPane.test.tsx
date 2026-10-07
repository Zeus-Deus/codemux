/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useAppStore } from "@/stores/app-store";

const mocks = vi.hoisted(() => ({
  startBrowserStream: vi.fn(),
  agentBrowserRun: vi.fn(),
  copyToClipboard: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  startBrowserStream: (...a: unknown[]) => mocks.startBrowserStream(...a),
  agentBrowserRun: (...a: unknown[]) => mocks.agentBrowserRun(...a),
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
  static opened = 0;
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(public url: string) {
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
  mocks.agentBrowserRun.mockReset().mockResolvedValue(null);
  mocks.copyToClipboard.mockReset();
  mocks.toastSuccess.mockReset();
  mocks.toastError.mockReset();
  FakeWebSocket.sent = [];
  FakeWebSocket.opened = 0;
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
