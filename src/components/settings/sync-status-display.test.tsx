/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
import { useAuthStore } from "@/stores/auth-store";

beforeEach(() => {
  useAuthStore.setState({ sessionStatus: "verified" });
});

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
    expect(container.querySelector('[class*="animate-pulse"]')).not.toBeNull();
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

  it.each(["verified", "offline"] as const)(
    "renders network failures neutrally while the auth session is %s",
    async (sessionStatus) => {
      useAuthStore.setState({ sessionStatus });
      vi.mocked(skillsSyncStatus).mockResolvedValue({
        state: "error",
        lastError: "network: error sending request for url (https://example.com/api/skills)",
        atMillis: Date.now(),
      });
      render(<SyncStatusDisplay />);

      expect(await screen.findByRole("status")).toHaveTextContent(/offline.+cached settings/i);
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Sync error")).not.toBeInTheDocument();
      expect(screen.queryByRole("button")).not.toBeInTheDocument();
    },
  );

  it("shows offline status while the initial skills status is pending", () => {
    useAuthStore.setState({ sessionStatus: "offline" });
    vi.mocked(skillsSyncStatus).mockReturnValue(new Promise(() => {}));
    render(<SyncStatusDisplay />);
    expect(screen.getByRole("status")).toHaveTextContent(/offline.+cached settings/i);
  });

  it.each([
    "list_skills: HTTP 401 Unauthorized",
    "list_skills: HTTP 500 Internal Server Error",
    "parse list_skills: invalid JSON",
    "write tmp mapping: permission denied",
    "list_skills: HTTP 403: network policy forbids access",
    "unknown sync failure",
  ])("keeps actionable errors visible even offline: %s", async (lastError) => {
    useAuthStore.setState({ sessionStatus: "offline" });
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "error",
      lastError,
      atMillis: Date.now(),
    });
    render(<SyncStatusDisplay />);
    expect(await screen.findByRole("alert")).toHaveTextContent(lastError);
    expect(screen.getByText("Sync error")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("recovers from offline after an auth update and successful sync event", async () => {
    useAuthStore.setState({ sessionStatus: "offline" });
    vi.mocked(skillsSyncStatus).mockResolvedValue({
      state: "error",
      lastError: "network: connection refused",
      atMillis: Date.now(),
    });
    render(<SyncStatusDisplay />);
    await screen.findByRole("status");
    act(() => {
      useAuthStore.setState({ sessionStatus: "verified" });
      capturedEventCallback!({ state: "idle", lastSyncAtMillis: Date.now() });
    });
    expect(await screen.findByText("Sync ready")).toBeInTheDocument();
    expect(screen.getByText(/Last synced just now/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sync now" })).toBeEnabled();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
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
