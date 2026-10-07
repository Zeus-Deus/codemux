/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ProviderHealthReport } from "@/tauri/types";
import { emptyHealthSlot, useProviderHealth } from "@/stores/provider-health-store";

import { SignInNotice } from "./SignInNotice";

const mockProbe = vi.fn(
  (_provider: string) => new Promise<ProviderHealthReport>(() => {}),
);
const mockOpenLogin = vi.fn((_workspaceId: string, _provider: string) =>
  Promise.resolve(),
);
vi.mock("@/tauri/commands", () => ({
  agentChatProviderHealth: (provider: string) => mockProbe(provider),
  openProviderLoginTerminal: (workspaceId: string, provider: string) =>
    mockOpenLogin(workspaceId, provider),
  getOrCreateHomeWorkspace: () => Promise.resolve("ws-home"),
  activateWorkspace: () => Promise.resolve(),
}));

// The pane's mount probe leaves a `ready` report with no expiry, and a
// signed-out run does not re-probe, so this is the state the card usually
// renders against.
function seedReadyClaude() {
  act(() => {
    useProviderHealth.setState({
      slots: {
        claude: {
          ...emptyHealthSlot(),
          report: {
            provider: "claude",
            status: "ready",
            installed: true,
            message: null,
            version: "2.1.0",
            login_command: null,
          },
          fetchedAt: Date.now(),
        },
        codex: emptyHealthSlot(),
        cursor: emptyHealthSlot(),
        grok: emptyHealthSlot(),
        hermes: emptyHealthSlot(),
        opencode: emptyHealthSlot(),
      },
    });
  });
}

describe("SignInNotice", () => {
  beforeEach(() => {
    mockProbe.mockClear();
    mockOpenLogin.mockClear();
    seedReadyClaude();
  });
  afterEach(cleanup);

  function renderNotice() {
    render(
      <SignInNotice
        provider="claude"
        message="Claude Code isn't signed in."
        workspaceId="ws-1"
      />,
    );
  }

  it("re-checks even when the cached report is a stale ready", () => {
    renderNotice();
    fireEvent.click(screen.getByRole("button", { name: "Re-check" }));
    expect(mockProbe).toHaveBeenCalledWith("claude");
  });

  it("opens the login terminal in the chat's workspace even when the cached report is ready", async () => {
    renderNotice();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    });
    expect(mockOpenLogin).toHaveBeenCalledWith("ws-1", "claude");
  });
});
