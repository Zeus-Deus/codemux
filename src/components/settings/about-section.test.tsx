/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn().mockResolvedValue("0.23.1") }));
vi.mock("@tauri-apps/api/path", () => ({
  appLogDir: vi.fn().mockResolvedValue("/data/com.codemux.app/logs"),
  join: vi.fn(async (...parts: string[]) => parts.join("/")),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
  revealItemInDir: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/tauri/commands", () => ({
  getPackageFormat: vi.fn().mockResolvedValue("other"),
}));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

let remoteClient = false;
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => remoteClient,
}));

const resetSettings = vi.fn().mockResolvedValue(undefined);
vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: (sel: (s: { resetSettings: typeof resetSettings }) => unknown) =>
    sel({ resetSettings }),
}));

import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  __resetUpdateStatusStoreForTests,
  useUpdateStatusStore,
} from "@/stores/update-status-store";
import { AboutSection } from "./about-section";

type Snapshot = Parameters<ReturnType<typeof useUpdateStatusStore.getState>["publish"]>[0];
function publish(overrides: Partial<Snapshot>) {
  useUpdateStatusStore.getState().publish({
    state: "idle",
    updateVersion: null,
    downloadProgress: 0,
    isRemote: false,
    startDownload: null,
    installAndRestart: null,
    requestDesktopUpdate: null,
    checkNow: null,
    lastCheck: null,
    ...overrides,
  });
}

describe("AboutSection", () => {
  beforeEach(() => __resetUpdateStatusStoreForTests());
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    remoteClient = false;
  });

  it("shows the running version and opens its release notes", async () => {
    render(<AboutSection />);
    expect(await screen.findByText(/Codemux v0\.23\.1/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Release notes/ }));
    // Vitest runs with DEV set, where the notes link is the release list.
    expect(openUrl).toHaveBeenCalledWith("https://github.com/Zeus-Deus/codemux/releases");
  });

  it("checks for updates on demand and only then claims the build is current", () => {
    const checkNow = vi.fn();
    publish({ checkNow });
    const view = render(<AboutSection />);
    expect(screen.queryByText(/latest version/)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(checkNow).toHaveBeenCalledTimes(1);

    publish({ checkNow, lastCheck: { at: Date.now(), ok: true } });
    view.rerender(<AboutSection />);
    expect(screen.getByText(/You're on the latest version/)).toBeInTheDocument();
  });

  it("reports a check that could not reach the server", () => {
    publish({ checkNow: vi.fn(), lastCheck: { at: Date.now(), ok: false } });
    render(<AboutSection />);
    expect(screen.getByText(/Couldn't reach the update server/)).toBeInTheDocument();
  });

  it("sends package-manager installs to the release page instead of downloading", () => {
    const startDownload = vi.fn();
    publish({ state: "update-available", updateVersion: "0.24.0", startDownload });
    render(<AboutSection />);
    expect(screen.getByText("Version 0.24.0 is available.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open release page" }));
    expect(startDownload).not.toHaveBeenCalled();
    expect(openUrl).toHaveBeenCalledWith(
      "https://github.com/Zeus-Deus/codemux/releases/tag/v0.24.0",
    );
  });

  it("falls back to the release list when the new version is unknown", () => {
    publish({ state: "update-available", updateVersion: null });
    render(<AboutSection />);
    fireEvent.click(screen.getByRole("button", { name: "Open release page" }));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/Zeus-Deus/codemux/releases");
  });

  it("offers the restart once an update is installed", () => {
    const installAndRestart = vi.fn();
    publish({ state: "ready", installAndRestart });
    render(<AboutSection />);
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    expect(installAndRestart).toHaveBeenCalledTimes(1);
  });

  it("reveals the log file", async () => {
    render(<AboutSection />);
    fireEvent.click(screen.getByRole("button", { name: "Show log file" }));
    await waitFor(() =>
      expect(revealItemInDir).toHaveBeenCalledWith("/data/com.codemux.app/logs/codemux.log"),
    );
  });

  it("resets synced settings only after confirming", async () => {
    render(<AboutSection />);
    fireEvent.click(screen.getByRole("button", { name: "Reset…" }));
    expect(resetSettings).not.toHaveBeenCalled();
    expect(
      await screen.findByText(/Custom themes and custom source-control hosts are deleted/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reset settings" }));
    expect(resetSettings).toHaveBeenCalledTimes(1);
  });

  it("hides host-only actions on the web remote", async () => {
    remoteClient = true;
    render(<AboutSection />);
    expect(await screen.findByText(/Codemux v0\.23\.1/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show log file" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reset…" })).toBeNull();
  });
});
