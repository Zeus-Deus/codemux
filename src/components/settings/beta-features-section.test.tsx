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

import { BetaFeaturesSection } from "./beta-features-section";

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

afterEach(() => cleanup());

function resetStore(enabled: boolean) {
  useFeatureFlags.setState({
    enableAgentChat: enabled,
    enableLazyWorkspaceCreation: enabled,
    loaded: true,
  });
}

describe("BetaFeaturesSection", () => {
  let reloadSpy: ReturnType<typeof vi.fn>;
  let originalLocation: Location;

  beforeEach(() => {
    invokeMock.mockReset();
    vi.mocked(toast.success).mockReset();
    vi.mocked(toast.error).mockReset();
    resetStore(false);

    // Stub window.location.reload so the test doesn't actually
    // trigger a navigation.
    originalLocation = window.location;
    reloadSpy = vi.fn();
    Object.defineProperty(window, "location", {
      configurable: true,
      value: {
        ...originalLocation,
        reload: reloadSpy,
      },
    });
  });

  afterEach(() => {
    Object.defineProperty(window, "location", {
      configurable: true,
      value: originalLocation,
    });
  });

  it("renders headline + Beta badge + description in OFF state", () => {
    render(<BetaFeaturesSection />);

    expect(screen.getByText(/Beta Features/i)).toBeInTheDocument();
    expect(screen.getByText("Agent Chat")).toBeInTheDocument();
    // The badge sits next to the "Agent Chat" label; use exact-match
    // so the section header's "Beta Features" doesn't double-count.
    expect(screen.getByText("Beta")).toBeInTheDocument();
    // Description copy includes the "off by default" hint.
    expect(
      screen.getByText(/your existing CLI workflow is unchanged/i),
    ).toBeInTheDocument();
  });

  it('shows the Switch unchecked when the toggle is off', () => {
    resetStore(false);
    render(<BetaFeaturesSection />);
    const sw = screen.getByRole("switch");
    expect(sw).toHaveAttribute("data-state", "unchecked");
  });

  it("shows the Switch checked when the toggle is on", () => {
    resetStore(true);
    render(<BetaFeaturesSection />);
    const sw = screen.getByRole("switch");
    expect(sw).toHaveAttribute("data-state", "checked");
  });

  it('"What\'s included" expander toggles the details list', async () => {
    const user = userEvent.setup();
    render(<BetaFeaturesSection />);

    expect(screen.queryByText(/MCP server runtime and management/i)).toBeNull();

    await user.click(screen.getByRole("button", { name: /What's included/ }));
    expect(
      screen.getByText(/MCP server runtime and management/i),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Hide details/ }));
    expect(screen.queryByText(/MCP server runtime and management/i)).toBeNull();
  });

  it("flipping the toggle invokes set_agent_chat_beta and schedules a reload", async () => {
    const user = userEvent.setup();
    invokeMock.mockResolvedValueOnce(undefined);

    render(<BetaFeaturesSection />);

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_agent_chat_beta", {
        enabled: true,
      }),
    );
    expect(toast.success).toHaveBeenCalledWith(
      expect.stringMatching(/enabled/i),
    );

    // Reload is scheduled via setTimeout(300). Wait for it instead of
    // mucking with fake timers (which interact poorly with userEvent's
    // own deferred scheduling).
    await waitFor(() => expect(reloadSpy).toHaveBeenCalledTimes(1), {
      timeout: 1500,
    });
  });

  it("surfaces a toast.error and does not reload when the backend fails", async () => {
    const user = userEvent.setup();
    invokeMock.mockRejectedValueOnce(new Error("disk full"));

    render(<BetaFeaturesSection />);

    await user.click(screen.getByRole("switch"));

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        expect.stringMatching(/Failed to update Agent Chat/i),
      ),
    );
    // No reload should fire even after the 300ms window passes.
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(reloadSpy).not.toHaveBeenCalled();
  });
});
