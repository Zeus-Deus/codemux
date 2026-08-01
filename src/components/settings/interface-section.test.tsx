/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { toast } from "@/lib/toast";
import { useFeatureFlags } from "@/stores/feature-flags";

import { InterfaceSection } from "./interface-section";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

afterEach(() => cleanup());

function resetStore(enabled: boolean) {
  useFeatureFlags.setState({
    enableAgentChat: enabled,
    enableLazyWorkspaceCreation: enabled,
    loaded: true,
  });
}

describe("InterfaceSection", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    // Default: every invoke resolves cleanly (each test that needs a
    // specific sequence overrides via `mockResolvedValueOnce` etc.).
    invokeMock.mockResolvedValue(undefined);
    vi.mocked(toast.success).mockReset();
    vi.mocked(toast.error).mockReset();
    vi.mocked(toast.warning).mockReset();
    // GUI is the default interface — tests start from the default-on
    // state unless they opt out explicitly.
    resetStore(true);
  });

  it("renders headline + toggle description without any Beta framing", () => {
    render(<InterfaceSection />);

    expect(screen.getByText("Interface")).toBeInTheDocument();
    expect(screen.getByText("Agent Chat GUI")).toBeInTheDocument();
    // The GUI is a promoted default, not a preview surface.
    expect(screen.queryByText(/beta/i)).toBeNull();
    // Description copy explains the way back to the CLI view.
    expect(
      screen.getByText(/classic terminal-first/i),
    ).toBeInTheDocument();
  });

  it("shows the Switch checked in the default (GUI-on) state", () => {
    resetStore(true);
    render(<InterfaceSection />);
    const sw = screen.getByRole("switch");
    expect(sw).toHaveAttribute("data-state", "checked");
  });

  it("shows the Switch unchecked when the user opted back into CLI", () => {
    resetStore(false);
    render(<InterfaceSection />);
    const sw = screen.getByRole("switch");
    expect(sw).toHaveAttribute("data-state", "unchecked");
  });

  it('"What\'s included" expander toggles the details list', async () => {
    const user = userEvent.setup();
    render(<InterfaceSection />);

    expect(screen.queryByText(/MCP server runtime and management/i)).toBeNull();

    await user.click(screen.getByRole("button", { name: /What's included/ }));
    expect(
      screen.getByText(/MCP server runtime and management/i),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Hide details/ }));
    expect(screen.queryByText(/MCP server runtime and management/i)).toBeNull();
  });

  it("flipping the toggle off persists the flag and quits the app", async () => {
    const user = userEvent.setup();

    render(<InterfaceSection />);

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_agent_chat_enabled", {
        enabled: false,
      }),
    );
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/Codemux will close. Reopen to apply/i),
      expect.objectContaining({ duration: expect.any(Number) }),
    );

    // The quit fires via setTimeout(600). We picked a plain quit
    // over an auto-restart because the auto-restart machinery
    // (detached spawn, setsid, /dev/null stdio, control-socket
    // teardown) doesn't survive the dev-server WebView path, and a
    // "dev: warn / prod: auto-restart" split confused the smoke
    // pass. Manual reopen is the honest UX in both modes.
    await waitFor(
      () => expect(invokeMock).toHaveBeenCalledWith("quit_app"),
      { timeout: 1500 },
    );
  });

  it("surfaces a toast.error and does NOT quit when set_agent_chat_enabled fails", async () => {
    const user = userEvent.setup();
    invokeMock.mockReset();
    invokeMock.mockRejectedValueOnce(new Error("disk full"));

    render(<InterfaceSection />);

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        expect.stringMatching(/Failed to update the interface setting/i),
      ),
    );
    // No quit_app call should fire even after the 600ms window
    // passes — only the failed set_agent_chat_enabled is in the log.
    // (Quitting after a failed persist would lose the user's intent
    // without persisting it.)
    await new Promise((resolve) => setTimeout(resolve, 800));
    expect(invokeMock).not.toHaveBeenCalledWith("quit_app");
  });

  it("surfaces a fallback toast if quit_app itself fails", async () => {
    const user = userEvent.setup();
    invokeMock.mockReset();
    // First call (set_agent_chat_enabled) succeeds; second call
    // (quit_app) rejects.
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "quit_app") {
        return Promise.reject(new Error("app handle gone"));
      }
      return Promise.resolve(undefined);
    });

    render(<InterfaceSection />);

    await user.click(screen.getByRole("switch"));

    await waitFor(
      () =>
        expect(toast.error).toHaveBeenCalledWith(
          expect.stringMatching(/please quit and reopen manually/i),
        ),
      { timeout: 1500 },
    );
  });
});
