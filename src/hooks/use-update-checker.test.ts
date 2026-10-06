import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, waitFor, cleanup, act } from "@testing-library/react";
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
  useUpdateChecker,
} from "./use-update-checker";
import {
  getPackageFormat,
  webRemoteStatus,
  webRemoteRequestUpdate,
  webRemotePublishUpdateAvailable,
} from "@/tauri/commands";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import { listen } from "@tauri-apps/api/event";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  useUpdateStatusStore,
  __resetUpdateStatusStoreForTests,
} from "@/stores/update-status-store";

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

  // A pacman/deb/rpm install cannot be replaced in place, so a browser's
  // "update & restart desktop" request must not start a doomed download.
  it("does not download on an install the updater cannot replace", () => {
    expect(updateAdvanceAction("update-available", true, false)).toBe("none");
  });

  it("does nothing without a real update or while mid-flight", () => {
    expect(updateAdvanceAction("update-available", false)).toBe("none");
    expect(updateAdvanceAction("idle", true)).toBe("none");
    expect(updateAdvanceAction("checking", true)).toBe("none");
    expect(updateAdvanceAction("downloading", true)).toBe("none");
  });
});

describe("useUpdateChecker → update status store mirror", () => {
  beforeEach(() => {
    __resetUpdateStatusStoreForTests();
    vi.mocked(webRemoteStatus).mockResolvedValue(
      status({ update_available: true, update_version: "9.9.9" }),
    );
    vi.mocked(onWebRemoteStateChanged).mockResolvedValue(() => {});
    vi.mocked(webRemoteRequestUpdate).mockResolvedValue(undefined);
    vi.mocked(listen).mockResolvedValue(() => {});
  });

  afterEach(() => {
    cleanup();
    delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    vi.clearAllMocks();
  });

  it("publishes the remote branch so a reader can act on a web client", async () => {
    // Without `isRemote` + `requestDesktopUpdate` in the mirror, the app-menu
    // footer would call `startDownload`, which is a no-op in a browser (there
    // is no updater plugin) — a dead "Update available" button.
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    renderHook(() => useUpdateChecker());

    await waitFor(() => {
      const s = useUpdateStatusStore.getState();
      expect(s.published).toBe(true);
      expect(s.state).toBe("update-available");
      expect(s.isRemote).toBe(true);
    });

    useUpdateStatusStore.getState().requestDesktopUpdate!();
    expect(webRemoteRequestUpdate).toHaveBeenCalledTimes(1);
  });

  it("publishes the desktop branch untouched", async () => {
    renderHook(() => useUpdateChecker());

    await waitFor(() => {
      const s = useUpdateStatusStore.getState();
      expect(s.published).toBe(true);
      expect(s.isRemote).toBe(false);
      expect(s.startDownload).toBeTypeOf("function");
    });
  });
});

describe("useUpdateChecker → desktop download failures", () => {
  type Win = { __CODEMUX_MOCK_UPDATER__?: boolean };
  const downloadAndInstall = vi.fn();

  beforeEach(() => {
    __resetUpdateStatusStoreForTests();
    vi.useFakeTimers();
    // Vitest runs with DEV set, where the native check is off unless the
    // browser mock opts in; the same switch drives it here.
    (window as Win).__CODEMUX_MOCK_UPDATER__ = true;
    vi.mocked(check).mockResolvedValue({
      version: "2.0.0",
      downloadAndInstall,
    } as unknown as Awaited<ReturnType<typeof check>>);
    vi.mocked(listen).mockResolvedValue(() => {});
    vi.mocked(webRemoteStatus).mockResolvedValue(status({}));
    vi.mocked(onWebRemoteStateChanged).mockResolvedValue(() => {});
    vi.mocked(webRemotePublishUpdateAvailable).mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    delete (window as Win).__CODEMUX_MOCK_UPDATER__;
    vi.clearAllMocks();
    downloadAndInstall.mockReset();
  });

  async function mountWithUpdate(format: string) {
    vi.mocked(getPackageFormat).mockResolvedValue(format);
    renderHook(() => useUpdateChecker());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(useUpdateStatusStore.getState().state).toBe("update-available");
  }

  it("keeps the failure reason and retries the download", async () => {
    downloadAndInstall
      .mockRejectedValueOnce("Download request failed with status: 404 Not Found")
      .mockResolvedValueOnce(undefined);
    await mountWithUpdate("appimage");

    await act(async () => {
      useUpdateStatusStore.getState().startDownload!();
    });
    let s = useUpdateStatusStore.getState();
    expect(s.state).toBe("error");
    expect(s.errorMessage).toBe(
      "Download request failed with status: 404 Not Found",
    );

    await act(async () => {
      useUpdateStatusStore.getState().retry!();
    });
    s = useUpdateStatusStore.getState();
    expect(downloadAndInstall).toHaveBeenCalledTimes(2);
    expect(s.state).toBe("ready");
    expect(s.errorMessage).toBeNull();
  });

  it("retries the restart, not the download, when relaunch failed", async () => {
    downloadAndInstall.mockResolvedValue(undefined);
    vi.mocked(relaunch).mockRejectedValueOnce(new Error("spawn failed"));
    await mountWithUpdate("appimage");

    await act(async () => {
      useUpdateStatusStore.getState().startDownload!();
    });
    await act(async () => {
      useUpdateStatusStore.getState().installAndRestart!();
    });
    expect(useUpdateStatusStore.getState().errorMessage).toBe("spawn failed");

    await act(async () => {
      useUpdateStatusStore.getState().retry!();
    });
    expect(relaunch).toHaveBeenCalledTimes(2);
    expect(downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("never downloads or announces to browsers on a pacman install", async () => {
    await mountWithUpdate("pacman");
    const s = useUpdateStatusStore.getState();
    expect(s.canAutoUpdate).toBe(false);
    expect(s.packageFormat).toBe("pacman");

    await act(async () => {
      s.startDownload!();
    });
    expect(downloadAndInstall).not.toHaveBeenCalled();
    expect(useUpdateStatusStore.getState().state).toBe("update-available");
    expect(webRemotePublishUpdateAvailable).not.toHaveBeenCalled();
  });

  it("announces auto-updatable installs to paired browsers", async () => {
    await mountWithUpdate("appimage");
    expect(webRemotePublishUpdateAvailable).toHaveBeenCalledWith(true, "2.0.0");
  });
});
