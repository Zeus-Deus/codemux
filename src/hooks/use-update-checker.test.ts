import { describe, it, expect, vi } from "vitest";
import type { WebRemoteStatus } from "@/tauri/types";

// The hook module imports Tauri plugins + web-remote helpers at the top level.
// Stub them so the pure helpers under test can be imported without a Tauri
// runtime present.
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@/tauri/commands", () => ({
  getPackageFormat: vi.fn(),
  webRemoteStatus: vi.fn(),
  webRemotePublishUpdateAvailable: vi.fn(),
  webRemoteRequestUpdate: vi.fn(),
}));
vi.mock("@/remote/web-remote-events", () => ({ onWebRemoteStateChanged: vi.fn() }));

import {
  canAutoUpdateFormat,
  isVersionDismissed,
  remoteClientsAttached,
  desktopUpdateFromStatus,
  updateAdvanceAction,
} from "./use-update-checker";

/** Build a full `WebRemoteStatus` for the defer/availability decision tests. */
function status(overrides: Partial<WebRemoteStatus>): WebRemoteStatus {
  return {
    enabled: true,
    running: true,
    port: 4377,
    require_approval: false,
    active_connections: 0,
    connected_sessions: 0,
    sessions: [],
    update_available: false,
    update_version: null,
    ...overrides,
  };
}

describe("canAutoUpdateFormat", () => {
  it("treats AppImage installs as auto-updatable", () => {
    expect(canAutoUpdateFormat("appimage")).toBe(true);
  });

  // Regression guard: before this fix `get_package_format()` returned
  // "other" on Windows, so `canAutoUpdate` was false and every Windows
  // user was downgraded to a manual download-and-reinstall flow despite
  // the NSIS updater fully supporting in-app updates.
  it("treats Windows NSIS installs as auto-updatable", () => {
    expect(canAutoUpdateFormat("nsis")).toBe(true);
  });

  it("does not auto-update deb/rpm/other installs", () => {
    expect(canAutoUpdateFormat("other")).toBe(false);
    expect(canAutoUpdateFormat("deb")).toBe(false);
    expect(canAutoUpdateFormat("rpm")).toBe(false);
  });

  it("rejects empty and unknown formats", () => {
    expect(canAutoUpdateFormat("")).toBe(false);
    expect(canAutoUpdateFormat("snap")).toBe(false);
  });
});

describe("isVersionDismissed", () => {
  // The dismissed version is held in an in-memory ref that starts as null
  // on every launch — so a fresh launch always shows the toast again.
  it("shows the toast on a fresh launch (nothing dismissed yet)", () => {
    expect(isVersionDismissed(null, "0.5.1")).toBe(false);
  });

  it("hides the toast for a version dismissed this session", () => {
    expect(isVersionDismissed("0.5.1", "0.5.1")).toBe(true);
  });

  // If a newer version is published while the app is running, the toast
  // must reappear even though an older version was dismissed.
  it("re-shows the toast when a newer version is published", () => {
    expect(isVersionDismissed("0.5.0", "0.5.1")).toBe(false);
  });
});

describe("remoteClientsAttached (desktop defer decision)", () => {
  it("is false when there is no status", () => {
    expect(remoteClientsAttached(null)).toBe(false);
  });

  // Nothing to defer for when nobody is connected — desktop behaves exactly
  // as it does with the server off.
  it("is false when the server is enabled but no device is connected", () => {
    expect(remoteClientsAttached(status({ connected_sessions: 0 }))).toBe(false);
  });

  it("is true when the server is enabled and a device is connected", () => {
    expect(remoteClientsAttached(status({ connected_sessions: 1 }))).toBe(true);
    expect(remoteClientsAttached(status({ connected_sessions: 3 }))).toBe(true);
  });

  // A stale count while the server is disabled must never trip the hint.
  it("is false when the server is disabled regardless of the count", () => {
    expect(
      remoteClientsAttached(status({ enabled: false, connected_sessions: 2 })),
    ).toBe(false);
  });
});

describe("desktopUpdateFromStatus (web client availability)", () => {
  it("reports no update when the status omits or clears availability", () => {
    expect(desktopUpdateFromStatus(null)).toEqual({ available: false, version: null });
    expect(desktopUpdateFromStatus(status({ update_available: false }))).toEqual({
      available: false,
      version: null,
    });
  });

  it("reports the available version when the desktop published one", () => {
    expect(
      desktopUpdateFromStatus(
        status({ update_available: true, update_version: "1.4.0" }),
      ),
    ).toEqual({ available: true, version: "1.4.0" });
  });

  // Availability can be true before the version string is filled in.
  it("reports available with a null version when none is given", () => {
    expect(
      desktopUpdateFromStatus(status({ update_available: true, update_version: null })),
    ).toEqual({ available: true, version: null });
  });
});

describe("updateAdvanceAction (desktop handling of a web request)", () => {
  it("restarts when an update is already downloaded and ready", () => {
    expect(updateAdvanceAction("ready", true)).toBe("restart");
    // Ready state means the install already happened — restart even if the
    // in-memory Update handle was cleared.
    expect(updateAdvanceAction("ready", false)).toBe("restart");
  });

  it("starts the download when an update is available and in hand", () => {
    expect(updateAdvanceAction("update-available", true)).toBe("download");
  });

  it("does nothing without a real update or while mid-flight", () => {
    expect(updateAdvanceAction("update-available", false)).toBe("none");
    expect(updateAdvanceAction("idle", true)).toBe("none");
    expect(updateAdvanceAction("checking", true)).toBe("none");
    expect(updateAdvanceAction("downloading", true)).toBe("none");
  });
});
