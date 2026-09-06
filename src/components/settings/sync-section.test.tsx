/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  getSyncStatus: vi.fn(),
  // The auth-store imports a few extras at module load — stub them.
  checkAuth: vi.fn(),
  startOauthFlow: vi.fn(),
  signinEmail: vi.fn(),
  signupEmail: vi.fn(),
  signOut: vi.fn(),
  // Export / import surface
  exportSkillsToFile: vi.fn(),
  importSkillsFromFile: vi.fn(),
  getExportRecommendedFilename: vi
    .fn()
    .mockResolvedValue("codemux-skills-export-2026-04-29.json"),
  pickSaveFileDialog: vi.fn(),
  pickOpenFileDialog: vi.fn(),
  // SyncStatusDisplay subscribes via skillsSyncStatus + a Tauri
  // event listener. Default to a resolved idle state so the display
  // renders without timing out.
  skillsSyncStatus: vi.fn().mockResolvedValue({
    state: "idle",
    lastSyncAtMillis: null,
  }),
  skillsSyncNow: vi.fn(),
}));

// SyncStatusDisplay registers a Tauri event listener; return a no-op
// unlisten so the tests don't have to know about it.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import {
  exportSkillsToFile,
  importSkillsFromFile,
  pickSaveFileDialog,
  pickOpenFileDialog,
} from "@/tauri/commands";
import { useAuthStore } from "@/stores/auth-store";
import { SyncSection } from "./sync-section";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

beforeEach(() => {
  // Reset to a clean default state before each test.
  useAuthStore.setState({
    syncAvailable: false,
    authMethod: null,
    user: { id: "u1", email: "user@example.com", name: "Test", image: null },
    isAuthenticated: true,
    sessionStatus: "verified",
  });
});

describe("SyncSection — render fork", () => {
  it("renders 'Sync ready' + export/import when syncAvailable is true", async () => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
    render(<SyncSection />);
    // The "Sync ready" label lives inside SyncStatusDisplay which
    // awaits the initial skillsSyncStatus promise.
    expect(await screen.findByText(/sync ready/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /export skills locally/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /import skills from backup/i }),
    ).toBeInTheDocument();
  });

  it("never asks a GitHub OAuth user for a password", async () => {
    // The whole point of server-side sync: an OAuth user is sync-ready
    // and is never prompted to create a sync password.
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
    render(<SyncSection />);
    await screen.findByText(/sync ready/i);
    expect(screen.queryByLabelText(/password/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/set up sync/i)).not.toBeInTheDocument();
  });

  it("shows a sign-in hint when sync isn't available yet", () => {
    useAuthStore.setState({ syncAvailable: false, authMethod: null });
    render(<SyncSection />);
    expect(screen.getByText(/sign in to sync your skills/i)).toBeInTheDocument();
    // No password inputs anywhere in the signed-out state.
    expect(screen.queryByLabelText(/password/i)).not.toBeInTheDocument();
  });

  it.each([
    ["offline", /offline.+cached settings/i],
    ["degraded", /settings refresh failed/i],
  ] as const)("surfaces the %s cached-sync state", async (sessionStatus, copy) => {
    useAuthStore.setState({ syncAvailable: true, sessionStatus });
    render(<SyncSection />);
    expect(await screen.findByText(copy)).toBeInTheDocument();
  });
});

describe("SyncSection — export", () => {
  beforeEach(() => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
  });

  it("exports to the chosen path and reports the count", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce("/tmp/backup.json");
    vi.mocked(exportSkillsToFile).mockResolvedValueOnce({
      path: "/tmp/backup.json",
      skillCount: 3,
      bytesWritten: 1234,
      failedCount: 0,
    });

    render(<SyncSection />);
    await screen.findByText(/sync ready/i);
    await userEvent.click(
      screen.getByRole("button", { name: /export skills locally/i }),
    );

    expect(exportSkillsToFile).toHaveBeenCalledWith("/tmp/backup.json");
    expect(await screen.findByText(/exported 3 skills/i)).toBeInTheDocument();
  });

  it("does nothing when the save dialog is cancelled", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce(null);
    render(<SyncSection />);
    await screen.findByText(/sync ready/i);
    await userEvent.click(
      screen.getByRole("button", { name: /export skills locally/i }),
    );
    expect(exportSkillsToFile).not.toHaveBeenCalled();
  });
});

describe("SyncSection — import", () => {
  beforeEach(() => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
  });

  it("imports from the chosen file and reports the count", async () => {
    vi.mocked(pickOpenFileDialog).mockResolvedValueOnce("/tmp/backup.json");
    vi.mocked(importSkillsFromFile).mockResolvedValueOnce({
      queuedCount: 2,
      failedCount: 0,
      mismatchedEmail: false,
    });

    render(<SyncSection />);
    await screen.findByText(/sync ready/i);
    await userEvent.click(
      screen.getByRole("button", { name: /import skills from backup/i }),
    );

    expect(importSkillsFromFile).toHaveBeenCalledWith("/tmp/backup.json");
    expect(await screen.findByText(/re-pushed 2 skills/i)).toBeInTheDocument();
  });

  it("surfaces the mismatched-account warning", async () => {
    vi.mocked(pickOpenFileDialog).mockResolvedValueOnce("/tmp/backup.json");
    vi.mocked(importSkillsFromFile).mockResolvedValueOnce({
      queuedCount: 1,
      failedCount: 0,
      mismatchedEmail: true,
    });

    render(<SyncSection />);
    await screen.findByText(/sync ready/i);
    await userEvent.click(
      screen.getByRole("button", { name: /import skills from backup/i }),
    );

    expect(
      await screen.findByText(/different account/i),
    ).toBeInTheDocument();
  });
});
