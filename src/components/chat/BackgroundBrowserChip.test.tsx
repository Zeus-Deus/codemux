/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import type { AgentBrowserSession } from "@/tauri/types";

import { BackgroundBrowserChip } from "./BackgroundBrowserChip";

function makeSession(overrides: Partial<AgentBrowserSession> = {}): AgentBrowserSession {
  return {
    session_id: "abs-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-abc123",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com/dashboard",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
    ...overrides,
  };
}

beforeEach(() => {
  useBrowserPeekStore.setState({ openWorkspaceId: null });
});

afterEach(() => cleanup());

describe("BackgroundBrowserChip", () => {
  it("shows a compact work-log event with the URL and navigating suffix", () => {
    render(<BackgroundBrowserChip session={makeSession()} workspaceId="ws-1" />);
    expect(screen.getByText("work log")).toBeInTheDocument();
    expect(screen.getByText("Opened the browser")).toBeInTheDocument();
    expect(
      screen.getByText(/https:\/\/example\.com\/dashboard.*navigating/),
    ).toBeInTheDocument();
    expect(screen.getByText("View")).toBeInTheDocument();
  });

  it("drops the navigating suffix once the session is no longer active", () => {
    render(
      <BackgroundBrowserChip
        session={makeSession({ is_active: false })}
        workspaceId="ws-1"
      />,
    );
    expect(screen.getByText("https://example.com/dashboard")).toBeInTheDocument();
  });

  it("can join an existing work-log stretch without repeating the label", () => {
    render(
      <BackgroundBrowserChip
        session={makeSession()}
        workspaceId="ws-1"
        showLabel={false}
      />,
    );
    expect(screen.queryByText("work log")).not.toBeInTheDocument();
  });

  it("opens the peek overlay for this workspace on click", async () => {
    render(<BackgroundBrowserChip session={makeSession()} workspaceId="ws-1" />);
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
    await userEvent.click(screen.getByText("Opened the browser"));
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(true);
  });
});
