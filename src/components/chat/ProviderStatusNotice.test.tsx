/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ProviderHealthReport } from "@/tauri/types";
import {
  HEALTH_REPROBE_MS,
  emptyHealthSlot,
  useProviderHealth,
} from "@/stores/provider-health-store";

import { ProviderStatusNotice } from "./ProviderStatusNotice";

// The store's refresh() invokes the Tauri command; stub it so tests
// control the report content directly through store state.
const mockProbe = vi.fn(
  (_provider: string) => new Promise<ProviderHealthReport>(() => {}),
);
vi.mock("@/tauri/commands", () => ({
  agentChatProviderHealth: (provider: string) => mockProbe(provider),
}));

// Mirrors the store's own refresh() semantics: a ready report clears
// the dismissal, a failure preserves whatever the user dismissed.
function seedReport(report: ProviderHealthReport) {
  act(() => {
    useProviderHealth.setState((state) => ({
      slots: {
        ...state.slots,
        [report.provider]: {
          ...state.slots[report.provider],
          report,
          fetchedAt: Date.now(),
          inFlight: null,
          dismissedKey:
            report.status === "ready"
              ? null
              : state.slots[report.provider].dismissedKey,
        },
      },
    }));
  });
}

function resetStore() {
  mockProbe.mockClear();
  act(() => {
    useProviderHealth.setState({
      slots: {
        claude: emptyHealthSlot(),
        codex: emptyHealthSlot(),
        cursor: emptyHealthSlot(),
        grok: emptyHealthSlot(),
        opencode: emptyHealthSlot(),
      },
    });
  });
}

describe("ProviderStatusNotice", () => {
  beforeEach(resetStore);
  afterEach(cleanup);

  it("does not probe provider health merely because a surface mounted", () => {
    render(<ProviderStatusNotice provider="claude" />);

    expect(mockProbe).not.toHaveBeenCalled();
  });

  it("renders nothing while the provider is healthy or unprobed", () => {
    render(<ProviderStatusNotice provider="claude" />);
    expect(
      screen.queryByTestId("provider-status-notice"),
    ).not.toBeInTheDocument();

    seedReport({
      provider: "claude",
      status: "ready",
      installed: true,
      message: null,
      version: "2.0.0",
    });
    expect(
      screen.queryByTestId("provider-status-notice"),
    ).not.toBeInTheDocument();
  });

  it("banners an error report with the probe message", () => {
    render(<ProviderStatusNotice provider="claude" />);
    seedReport({
      provider: "claude",
      status: "error",
      installed: true,
      message: "Claude Agent runtime is installed but failed to run.",
      version: null,
    });
    const banner = screen.getByTestId("provider-status-notice");
    expect(banner).toHaveTextContent("Claude");
    expect(banner).toHaveTextContent(
      "Claude Agent runtime is installed but failed to run.",
    );
  });

  it("uses the Grok label for Grok health failures", () => {
    render(<ProviderStatusNotice provider="grok" />);
    seedReport({
      provider: "grok",
      status: "error",
      installed: false,
      message: "Grok CLI is not installed or not on PATH.",
      version: null,
    });
    expect(screen.getByTestId("provider-status-notice")).toHaveTextContent(
      "Grok",
    );
  });

  it("dismiss hides the banner until the failure changes", () => {
    render(<ProviderStatusNotice provider="codex" />);
    seedReport({
      provider: "codex",
      status: "error",
      installed: false,
      message: "Codex CLI (`codex`) is not installed or not on PATH.",
      version: null,
    });
    fireEvent.click(
      screen.getByRole("button", { name: /dismiss codex provider status/i }),
    );
    expect(
      screen.queryByTestId("provider-status-notice"),
    ).not.toBeInTheDocument();

    // Same failure again → stays dismissed.
    seedReport({
      provider: "codex",
      status: "error",
      installed: false,
      message: "Codex CLI (`codex`) is not installed or not on PATH.",
      version: null,
    });
    expect(
      screen.queryByTestId("provider-status-notice"),
    ).not.toBeInTheDocument();

    // A DIFFERENT failure re-banners.
    seedReport({
      provider: "codex",
      status: "error",
      installed: true,
      message: "Codex CLI is not authenticated.",
      version: null,
    });
    expect(screen.getByTestId("provider-status-notice")).toHaveTextContent(
      "Codex CLI is not authenticated.",
    );
  });

  it("re-probes an unhealthy provider on the recovery cadence", () => {
    vi.useFakeTimers();
    try {
      render(<ProviderStatusNotice provider="claude" />);
      const afterMount = mockProbe.mock.calls.length;
      expect(afterMount).toBe(0);

      // Healthy/unprobed: nothing to recover from, so the poll must not
      // spawn a CLI check every few minutes for every mounted pane.
      act(() => {
        vi.advanceTimersByTime(HEALTH_REPROBE_MS * 2);
      });
      expect(mockProbe.mock.calls.length).toBe(afterMount);

      // Unhealthy: the user may have fixed it in a terminal, and nothing
      // else in the app would notice — poll so the banner clears itself.
      seedReport({
        provider: "claude",
        status: "error",
        installed: true,
        message: "Claude CLI is not authenticated.",
        version: null,
      });
      act(() => {
        vi.advanceTimersByTime(HEALTH_REPROBE_MS);
      });
      expect(mockProbe.mock.calls.length).toBe(afterMount + 1);
    } finally {
      vi.useRealTimers();
    }
  });
});
