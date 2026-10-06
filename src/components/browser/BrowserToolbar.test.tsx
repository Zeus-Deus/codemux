/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  agentBrowserRun: vi.fn(),
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/tauri/commands", () => ({
  agentBrowserRun: (...a: unknown[]) => mocks.agentBrowserRun(...a),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...a: unknown[]) => mocks.openUrl(...a),
}));

import { BrowserToolbar } from "./BrowserToolbar";

function renderToolbar(props: Partial<Parameters<typeof BrowserToolbar>[0]> = {}) {
  const onUrlChange = vi.fn();
  render(
    <BrowserToolbar
      browserId="browser-1"
      sessionId="ws-abc"
      currentUrl="about:blank"
      onUrlChange={onUrlChange}
      loading={false}
      inspectorActive={false}
      onInspectorToggle={() => {}}
      {...props}
    />,
  );
  return { onUrlChange, input: screen.getByLabelText("Address") };
}

beforeEach(() => {
  mocks.agentBrowserRun.mockReset().mockResolvedValue(null);
  mocks.openUrl.mockClear();
  useAppStore.setState({ appState: null });
});

afterEach(() => cleanup());

describe("BrowserToolbar", () => {
  it("opens a local dev server over http", async () => {
    const { input, onUrlChange } = renderToolbar();
    await userEvent.clear(input);
    await userEvent.type(input, "localhost:5173{Enter}");
    expect(mocks.agentBrowserRun).toHaveBeenCalledWith("ws-abc", "open", { url: "http://localhost:5173" });
    await vi.waitFor(() => expect(onUrlChange).toHaveBeenCalledWith("http://localhost:5173"));
  });

  it("shows a failed navigation inline and clears it on the next keystroke", async () => {
    mocks.agentBrowserRun.mockRejectedValueOnce("net::ERR_CONNECTION_REFUSED");
    const { input, onUrlChange } = renderToolbar();
    await userEvent.clear(input);
    await userEvent.type(input, "localhost:9999{Enter}");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't open http://localhost:9999: net::ERR_CONNECTION_REFUSED");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(onUrlChange).not.toHaveBeenCalled();
    // The typed address survives so it can be corrected.
    expect(input).toHaveValue("localhost:9999");

    await userEvent.type(input, "0");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(input).not.toHaveAttribute("aria-invalid");
  });

  it("offers Stop while a navigation is in flight", async () => {
    mocks.agentBrowserRun.mockImplementation((_id: string, action: string) =>
      action === "open" ? new Promise(() => {}) : Promise.resolve(null),
    );
    const { input, onUrlChange } = renderToolbar();
    await userEvent.clear(input);
    await userEvent.type(input, "example.com{Enter}");

    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));
    expect(mocks.agentBrowserRun).toHaveBeenCalledWith("ws-abc", "eval", { script: "window.stop()" });
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    expect(onUrlChange).not.toHaveBeenCalled();
  });

  it("does not offer a reload while the stream is still connecting", () => {
    renderToolbar({ loading: true });
    expect(screen.getByRole("button", { name: "Connecting" })).toBeDisabled();
  });

  it("suggests the active workspace's detected ports", () => {
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [],
        detected_ports: [
          { port: 5173, pid: 1, process_name: "vite", workspace_id: "ws-1", label: null, source: null },
          { port: 5432, pid: 2, process_name: "postgres", workspace_id: null, label: null, source: null },
        ],
      } as unknown as AppStateSnapshot,
    });
    const { input } = renderToolbar();
    const listId = input.getAttribute("list");
    expect(listId).toBeTruthy();
    const options = Array.from(document.getElementById(listId!)!.querySelectorAll("option"));
    expect(options.map((o) => o.value)).toEqual(["http://localhost:5173"]);
  });
});
