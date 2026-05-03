/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  skillsSyncNow: vi.fn(),
  skillsSyncStatus: vi.fn(),
}));

// Tauri's `listen` lives in @tauri-apps/api/event. Default mock
// returns an unlisten that's a no-op; individual tests can
// override to inject payloads via the captured callback.
let capturedEventCallback:
  | ((payload: Record<string, unknown>) => void)
  | null = null;

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, cb: (e: { payload: unknown }) => void) => {
    capturedEventCallback = (payload) => cb({ payload });
    return () => {
      capturedEventCallback = null;
    };
  }),
}));

import { skillsSyncNow, skillsSyncStatus } from "@/tauri/commands";
import { SyncStatusDisplay, SyncStateIcon } from "./sync-status-display";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  capturedEventCallback = null;
});

describe("SyncStateIcon", () => {
  it("renders the success icon with an aria-label for state=idle", () => {
    render(<SyncStateIcon state="idle" />);
    expect(screen.getByLabelText("Sync ready")).toBeInTheDocument();
  });

  it("renders the spinner with an aria-label for state=syncing", () => {
    render(<SyncStateIcon state="syncing" />);
    expect(screen.getByLabelText("Syncing")).toBeInTheDocument();
  });

  it("renders the error icon with an aria-label for state=error", () => {
    render(<SyncStateIcon state="error" />);
    expect(screen.getByLabelText("Sync error")).toBeInTheDocument();
  });
});

describe("SyncStatusDisplay", () => {
  it("renders a skeleton while initial fetch is pending", () => {
    vi.mocked(skillsSyncStatus).mockReturnValue(new Promise(() => {})); // never resolves
    const { container } = render(<SyncStatusDisplay />);
    expect(container.querySelector(".animate-pulse")).not.toBeNull();
  });

  it("idle with no last sync hides the relative-time line but shows the label", async () => {
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "idle",
      lastSyncAtMillis: null,
    });
    render(<SyncStatusDisplay />);

    await waitFor(() => {
      expect(screen.getByText("Sync ready")).toBeInTheDocument();
    });
    expect(screen.queryByText(/Last synced/)).toBeNull();
    expect(screen.getByRole("button", { name: /sync now/i })).toBeEnabled();
  });

  it("idle with a recent last sync renders the relative-time line", async () => {
    const oneMinuteAgo = Date.now() - 60_000;
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "idle",
      lastSyncAtMillis: oneMinuteAgo,
    });
    render(<SyncStatusDisplay />);

    expect(
      await screen.findByText(/Last synced 1 minute ago/i),
    ).toBeInTheDocument();
  });

  it("syncing state disables the sync button + shows spinner", async () => {
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "syncing",
      startedAtMillis: Date.now(),
    });
    render(<SyncStatusDisplay />);

    await waitFor(() => {
      expect(screen.getByText(/Syncing…/)).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: /sync now/i })).toBeDisabled();
  });

  it("error state shows error banner and a Retry button", async () => {
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "error",
      lastError: "Network unreachable",
      atMillis: Date.now(),
    });
    render(<SyncStatusDisplay />);

    expect(await screen.findByText("Sync error")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(/Network unreachable/);
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("Sync now button calls skillsSyncNow and disables until post-cycle event arrives", async () => {
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "idle",
      lastSyncAtMillis: null,
    });
    vi.mocked(skillsSyncNow).mockResolvedValue({
      pushedCount: 0,
      pulledCount: 0,
      conflictCount: 0,
      errorCount: 0,
    });

    render(<SyncStatusDisplay />);
    const button = await screen.findByRole("button", { name: /sync now/i });
    await userEvent.click(button);

    expect(skillsSyncNow).toHaveBeenCalled();

    // The hook's `optimisticSyncing` flag is cleared by any
    // non-syncing event payload — that's the contract with the
    // Tauri command wrapper which emits one before and one after
    // the engine's pull/push cycle. Simulate the post-cycle
    // "idle" event here.
    expect(capturedEventCallback).not.toBeNull();
    act(() => {
      capturedEventCallback!({ state: "idle", lastSyncAtMillis: Date.now() });
    });

    await waitFor(() => expect(button).not.toBeDisabled());
  });

  it("event-driven updates flip the rendered state without re-fetching", async () => {
    vi.useRealTimers();
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "idle",
      lastSyncAtMillis: null,
    });
    render(<SyncStatusDisplay />);
    await screen.findByText("Sync ready");

    expect(capturedEventCallback).not.toBeNull();
    act(() => {
      capturedEventCallback!({ state: "syncing", startedAtMillis: Date.now() });
    });
    expect(await screen.findByText(/Syncing…/)).toBeInTheDocument();

    act(() => {
      capturedEventCallback!({
        state: "error",
        lastError: "Server returned 500",
        atMillis: Date.now(),
      });
    });
    expect(await screen.findByText("Sync error")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("Server returned 500");
  });
});
