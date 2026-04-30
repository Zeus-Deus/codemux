/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/tauri/commands", () => ({
  setupSyncPassword: vi.fn(),
  providePasswordForSync: vi.fn(),
  getSyncStatus: vi.fn(),
  // The auth-store imports a few extras at module load — stub them.
  checkAuth: vi.fn(),
  startOauthFlow: vi.fn(),
  signinEmail: vi.fn(),
  signupEmail: vi.fn(),
  signOut: vi.fn(),
  // Stage 4 surface
  exportSkillsToFile: vi.fn(),
  importSkillsFromFile: vi.fn(),
  getExportRecommendedFilename: vi.fn().mockResolvedValue("codemux-skills-export-2026-04-29.json"),
  pickSaveFileDialog: vi.fn(),
  pickOpenFileDialog: vi.fn(),
  wipeRemoteSkillsForReset: vi.fn(),
  // Stage 5: SyncStatusDisplay subscribes via skillsSyncStatus +
  // a Tauri event listener. Default to a resolved idle state so
  // the display renders without timing out.
  skillsSyncStatus: vi.fn().mockResolvedValue({
    state: "idle",
    lastSyncAtMillis: null,
  }),
  skillsSyncNow: vi.fn(),
}));

// Stage 5 added a Tauri event listener inside SyncStatusDisplay.
// The default mock returns a no-op unlisten so the existing
// SyncSection tests don't have to know about it.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import {
  setupSyncPassword,
  providePasswordForSync,
  exportSkillsToFile,
  importSkillsFromFile,
  pickSaveFileDialog,
  pickOpenFileDialog,
  wipeRemoteSkillsForReset,
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
  });
});

describe("SyncSection — render fork", () => {
  it("renders 'Sync ready' when syncAvailable is true", async () => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
    render(<SyncSection />);
    // Stage 5: the "Sync ready" label now lives inside
    // SyncStatusDisplay which waits on the initial skillsSyncStatus
    // promise. Use findByText so the assertion awaits the resolution.
    expect(await screen.findByText(/sync ready/i)).toBeInTheDocument();
  });

  it("renders SetupSyncPasswordForm for GitHub OAuth user without key", () => {
    useAuthStore.setState({ syncAvailable: false, authMethod: "github" });
    render(<SyncSection />);
    // Stage 2 polish: heading is "Skills sync (optional)" with
    // explanatory copy below — not a single "Set up skills sync"
    // line. Match on the heading + explanation hooks.
    expect(screen.getByText(/skills sync/i)).toBeInTheDocument();
    expect(screen.getByText(/\(optional\)/i)).toBeInTheDocument();
    expect(screen.getByText(/end-to-end encrypted/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/^new password/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/confirm password/i)).toBeInTheDocument();
  });

  it("renders ProvidePasswordForm for email user without key", () => {
    useAuthStore.setState({ syncAvailable: false, authMethod: "email" });
    render(<SyncSection />);
    expect(
      screen.getByText(/re-enter your password to unlock sync/i),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/^password$/i)).toBeInTheDocument();
  });

  it("renders ProvidePasswordForm when authMethod is null (cold start)", () => {
    useAuthStore.setState({ syncAvailable: false, authMethod: null });
    render(<SyncSection />);
    expect(
      screen.getByText(/re-enter your password to unlock sync/i),
    ).toBeInTheDocument();
  });
});

describe("SetupSyncPasswordForm — validation", () => {
  beforeEach(() => {
    useAuthStore.setState({ syncAvailable: false, authMethod: "github" });
  });

  it("disables Set-up-sync until password meets requirements", async () => {
    render(<SyncSection />);
    const submit = screen.getByRole("button", { name: /set up sync/i });
    expect(submit).toBeDisabled();

    const newPw = screen.getByLabelText(/^new password/i);
    const confirm = screen.getByLabelText(/confirm password/i);
    const ack = screen.getByRole("checkbox");

    await userEvent.type(newPw, "shortpw1"); // 8 chars, has letter+digit
    await userEvent.type(confirm, "shortpw1");
    expect(submit).toBeDisabled(); // ack not yet checked
    await userEvent.click(ack);
    expect(submit).toBeEnabled();
  });

  it("disables submit when passwords don't match", async () => {
    render(<SyncSection />);
    const newPw = screen.getByLabelText(/^new password/i);
    const confirm = screen.getByLabelText(/confirm password/i);
    const ack = screen.getByRole("checkbox");

    await userEvent.type(newPw, "valid-password-1");
    await userEvent.type(confirm, "different-password-1");
    await userEvent.click(ack);
    expect(screen.getByRole("button", { name: /set up sync/i })).toBeDisabled();
  });

  it("rejects passwords without a digit", async () => {
    render(<SyncSection />);
    const newPw = screen.getByLabelText(/^new password/i);
    const confirm = screen.getByLabelText(/confirm password/i);
    const ack = screen.getByRole("checkbox");

    await userEvent.type(newPw, "all-letters-no-digit");
    await userEvent.type(confirm, "all-letters-no-digit");
    await userEvent.click(ack);
    expect(screen.getByRole("button", { name: /set up sync/i })).toBeDisabled();
  });

  it("rejects passwords shorter than 8 characters", async () => {
    render(<SyncSection />);
    const newPw = screen.getByLabelText(/^new password/i);
    const confirm = screen.getByLabelText(/confirm password/i);
    const ack = screen.getByRole("checkbox");

    await userEvent.type(newPw, "ab1");
    await userEvent.type(confirm, "ab1");
    await userEvent.click(ack);
    expect(screen.getByRole("button", { name: /set up sync/i })).toBeDisabled();
  });

  it("submits to setupSyncPassword and reflects new status", async () => {
    vi.mocked(setupSyncPassword).mockResolvedValueOnce({
      syncAvailable: true,
      authMethod: "github",
    });

    render(<SyncSection />);
    await userEvent.type(screen.getByLabelText(/^new password/i), "valid-pw-1234");
    await userEvent.type(screen.getByLabelText(/confirm password/i), "valid-pw-1234");
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /set up sync/i }));

    expect(setupSyncPassword).toHaveBeenCalledWith("valid-pw-1234");
    // After the resolved promise the store has been updated; the UI
    // re-renders into the "Sync ready" branch.
    expect(useAuthStore.getState().syncAvailable).toBe(true);
  });

  it("surfaces backend errors without losing the typed password", async () => {
    vi.mocked(setupSyncPassword).mockRejectedValueOnce("Server rejected password: HTTP 500");

    render(<SyncSection />);
    await userEvent.type(screen.getByLabelText(/^new password/i), "valid-pw-1234");
    await userEvent.type(screen.getByLabelText(/confirm password/i), "valid-pw-1234");
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /set up sync/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/server rejected/i);
    // The password input still holds the typed value so the user can retry.
    expect(screen.getByLabelText(/^new password/i)).toHaveValue("valid-pw-1234");
  });

  it("requires the recovery acknowledgment checkbox", async () => {
    render(<SyncSection />);
    await userEvent.type(screen.getByLabelText(/^new password/i), "valid-pw-1234");
    await userEvent.type(screen.getByLabelText(/confirm password/i), "valid-pw-1234");
    // No click on the ack checkbox.
    expect(screen.getByRole("button", { name: /set up sync/i })).toBeDisabled();
  });

  it("renders the 'permanently unrecoverable' warning copy verbatim", () => {
    render(<SyncSection />);
    expect(
      screen.getByText(/permanently unrecoverable/i),
    ).toBeInTheDocument();
  });
});

describe("SyncReadyRow — Stage 4 export / import / reset controls", () => {
  beforeEach(() => {
    useAuthStore.setState({ syncAvailable: true, authMethod: "github" });
  });

  it("renders Export, Import, and Forgot-password controls", () => {
    render(<SyncSection />);
    expect(screen.getByRole("button", { name: /export skills locally/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /import skills from backup/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /forgot your sync password/i })).toBeInTheDocument();
  });

  it("Export button: opens save dialog, calls exportSkillsToFile, surfaces toast text", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce("/tmp/export.json");
    vi.mocked(exportSkillsToFile).mockResolvedValueOnce({
      path: "/tmp/export.json",
      skillCount: 3,
      bytesWritten: 512,
      failedCount: 0,
    });

    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /export skills locally/i }));

    expect(pickSaveFileDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Export skills",
        defaultFilename: "codemux-skills-export-2026-04-29.json",
        filterName: "JSON",
        filterExtensions: ["json"],
      }),
    );
    expect(exportSkillsToFile).toHaveBeenCalledWith("/tmp/export.json");
    expect(await screen.findByRole("status")).toHaveTextContent(/exported 3 skills/i);
  });

  it("Export button: silently no-ops when the user cancels the save dialog", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce(null);

    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /export skills locally/i }));

    expect(pickSaveFileDialog).toHaveBeenCalled();
    expect(exportSkillsToFile).not.toHaveBeenCalled();
  });

  it("Import button: calls importSkillsFromFile and warns about email mismatch", async () => {
    vi.mocked(pickOpenFileDialog).mockResolvedValueOnce("/tmp/backup.json");
    vi.mocked(importSkillsFromFile).mockResolvedValueOnce({
      queuedCount: 5,
      failedCount: 0,
      mismatchedEmail: true,
    });

    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /import skills from backup/i }));

    expect(importSkillsFromFile).toHaveBeenCalledWith("/tmp/backup.json");
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent(/re-pushed 5 skills/i);
    expect(status).toHaveTextContent(/different account/i);
  });

  it("Import button: surfaces error feedback on failure", async () => {
    vi.mocked(pickOpenFileDialog).mockResolvedValueOnce("/tmp/bad.json");
    vi.mocked(importSkillsFromFile).mockRejectedValueOnce("Export file version 999 not supported");

    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /import skills from backup/i }));

    expect(await screen.findByRole("status")).toHaveTextContent(/import failed/i);
  });

  it("Forgot-password link opens the reset dialog", async () => {
    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /forgot your sync password/i }));
    // The first dialog step asks for confirmation — its destructive
    // button is the canary that the dialog actually opened.
    expect(
      await screen.findByRole("button", { name: /export skills first/i }),
    ).toBeInTheDocument();
  });
});

describe("ResetSyncPasswordDialog", () => {
  beforeEach(() => {
    useAuthStore.setState({
      syncAvailable: true,
      authMethod: "github",
      user: {
        id: "u1",
        email: "user@example.com",
        name: "Test",
        image: null,
      },
      isAuthenticated: true,
    });
  });

  async function openDialog() {
    render(<SyncSection />);
    await userEvent.click(screen.getByRole("button", { name: /forgot your sync password/i }));
  }

  it("warn step: shows the destructive copy and the three buttons", async () => {
    await openDialog();
    expect(await screen.findByText(/permanently delete every synced skill/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /export skills first/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /i don't need a backup/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^cancel$/i })).toBeInTheDocument();
  });

  it("export path: success transitions to 'Backup saved' and exposes Continue", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce("/tmp/export.json");
    vi.mocked(exportSkillsToFile).mockResolvedValueOnce({
      path: "/tmp/export.json",
      skillCount: 4,
      bytesWritten: 1024,
      failedCount: 0,
    });
    await openDialog();
    await userEvent.click(screen.getByRole("button", { name: /export skills first/i }));

    expect(await screen.findByText(/backup saved/i)).toBeInTheDocument();
    expect(screen.getByText(/saved 4 skills/i)).toBeInTheDocument();
    expect(screen.getByText("/tmp/export.json")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /continue with reset/i })).toBeInTheDocument();
  });

  it("skip path: extra acknowledgment checkbox required before destructive button enables", async () => {
    await openDialog();
    await userEvent.click(screen.getByRole("button", { name: /i don't need a backup/i }));

    const destructive = await screen.findByRole("button", {
      name: /wipe my skills and reset/i,
    });
    expect(destructive).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox"));
    expect(destructive).toBeEnabled();
  });

  it("Continue with reset: calls wipeRemoteSkillsForReset and lands on Done", async () => {
    vi.mocked(pickSaveFileDialog).mockResolvedValueOnce("/tmp/export.json");
    vi.mocked(exportSkillsToFile).mockResolvedValueOnce({
      path: "/tmp/export.json",
      skillCount: 1,
      bytesWritten: 256,
      failedCount: 0,
    });
    vi.mocked(wipeRemoteSkillsForReset).mockResolvedValueOnce(undefined);

    await openDialog();
    await userEvent.click(screen.getByRole("button", { name: /export skills first/i }));
    await userEvent.click(await screen.findByRole("button", { name: /continue with reset/i }));

    expect(wipeRemoteSkillsForReset).toHaveBeenCalled();
    expect(await screen.findByText(/check your email/i)).toBeInTheDocument();
    expect(screen.getByText(/user@example.com/)).toBeInTheDocument();
  });

  it("error step is reachable when wipe fails and offers retry", async () => {
    vi.mocked(wipeRemoteSkillsForReset).mockRejectedValueOnce("Server unreachable");

    await openDialog();
    // Take the no-backup path to skip the export step.
    await userEvent.click(screen.getByRole("button", { name: /i don't need a backup/i }));
    await userEvent.click(await screen.findByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /wipe my skills and reset/i }));

    expect(await screen.findByText(/something went wrong/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /try again/i })).toBeInTheDocument();
  });
});

describe("ProvidePasswordForm — repair flow", () => {
  beforeEach(() => {
    useAuthStore.setState({ syncAvailable: false, authMethod: "email" });
  });

  it("submits to providePasswordForSync and unlocks sync", async () => {
    vi.mocked(providePasswordForSync).mockResolvedValueOnce({
      syncAvailable: true,
      authMethod: "email",
    });

    render(<SyncSection />);
    await userEvent.type(screen.getByLabelText(/^password$/i), "user-typed-password");
    await userEvent.click(screen.getByRole("button", { name: /unlock sync/i }));

    expect(providePasswordForSync).toHaveBeenCalledWith("user-typed-password");
    expect(useAuthStore.getState().syncAvailable).toBe(true);
  });

  it("disables submit for passwords below 8 characters", async () => {
    render(<SyncSection />);
    await userEvent.type(screen.getByLabelText(/^password$/i), "abc");
    expect(screen.getByRole("button", { name: /unlock sync/i })).toBeDisabled();
  });

  it("does NOT include the acknowledgment checkbox (repair, not setup)", () => {
    render(<SyncSection />);
    // Only one form rendered — ProvidePasswordForm — and it has no
    // checkbox. The setup-time warning is not relevant for repair.
    expect(screen.queryByRole("checkbox")).toBeNull();
  });
});
