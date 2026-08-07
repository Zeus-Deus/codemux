import { useState, useEffect, useRef, useCallback } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { listen } from "@tauri-apps/api/event";
import {
  getPackageFormat,
  webRemoteStatus,
  webRemotePublishUpdateAvailable,
  webRemoteRequestUpdate,
} from "@/tauri/commands";
import { onWebRemoteStateChanged } from "@/remote/web-remote-events";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import type { WebRemoteStatus } from "@/tauri/types";

import { useUpdateStatusStore, type UpdateState } from "@/stores/update-status-store";

export type { UpdateState };

interface UpdateCheckerResult {
  state: UpdateState;
  updateVersion: string | null;
  downloadProgress: number;
  canAutoUpdate: boolean;
  startDownload: () => void;
  installAndRestart: () => void;
  dismiss: () => void;
  dismissed: boolean;
  /**
   * True on the web (remote) client. The remote client has no updater plugin,
   * so its toast is a "desktop update available → update & restart desktop"
   * variant driven entirely by {@link requestDesktopUpdate}.
   */
  isRemote: boolean;
  /**
   * Desktop only: at least one remote device is attached right now. When an
   * update is ready the toast surfaces a "remote devices are connected" hint so
   * restarting the desktop (which briefly disconnects those devices) is a
   * deliberate choice rather than an automatic one.
   */
  remoteClientsConnected: boolean;
  /** Web only: ask the desktop to run its update + restart flow. */
  requestDesktopUpdate: () => void;
  /** Web only: the user pressed "update & restart desktop". */
  updateRequested: boolean;
}

const CHECK_INTERVAL = 4 * 60 * 60 * 1000; // 4 hours
const INITIAL_DELAY = 5000; // 5 seconds

/**
 * Global event a paired web client raises to ask the desktop to update +
 * restart. Must match `UPDATE_REQUESTED_EVENT` in
 * `src-tauri/src/web_remote/mod.rs`.
 */
const UPDATE_REQUESTED_EVENT = "web-remote-update-requested";

/**
 * Install formats whose updates the Tauri updater can download and apply
 * in place. Everything else (deb/rpm/unknown) can only be pointed at the
 * release page. Keep this in sync with `get_package_format` in
 * `src-tauri/src/commands/update.rs`.
 */
const AUTO_UPDATABLE_FORMATS = new Set(["appimage", "nsis"]);

export function canAutoUpdateFormat(format: string): boolean {
  return AUTO_UPDATABLE_FORMATS.has(format);
}

/**
 * Whether the desktop should treat an update restart as "deferred by default"
 * because a paired browser is actively driving this instance. The desktop
 * updater keys its "remote devices are connected" hint off this. Pure over the
 * status snapshot so the defer decision is unit-testable: it is true only when
 * the server is enabled AND at least one distinct device has a live socket.
 */
export function remoteClientsAttached(status: WebRemoteStatus | null): boolean {
  return !!status && status.enabled && (status.connected_sessions ?? 0) > 0;
}

export interface DesktopUpdateInfo {
  available: boolean;
  version: string | null;
}

/**
 * Extract the desktop-update availability a web client shows from a server
 * status snapshot. Pure so the web toast's "is there an update?" decision is
 * testable without a live server.
 */
export function desktopUpdateFromStatus(
  status: WebRemoteStatus | null,
): DesktopUpdateInfo {
  if (!status || !status.update_available) {
    return { available: false, version: null };
  }
  return { available: true, version: status.update_version ?? null };
}

/**
 * What the desktop should do when a web client asks it to update, given the
 * desktop's current updater state. `ready` → restart; a fresh
 * `update-available` (with a real `Update` in hand) → start the download;
 * anything mid-flight (`checking`/`downloading`) or with nothing available →
 * do nothing. Pure so the request-handler branch is unit-testable.
 */
export function updateAdvanceAction(
  state: UpdateState,
  hasUpdate: boolean,
): "download" | "restart" | "none" {
  if (state === "ready") return "restart";
  if (state === "update-available" && hasUpdate) return "download";
  return "none";
}

/**
 * Whether the update toast should be treated as dismissed for `version`.
 *
 * Dismissal is intentionally in-memory only (a ref, not localStorage), so
 * "Later" hides the toast for the current session but it re-appears on the
 * next app launch — and immediately if a newer version is published —
 * until the user actually updates.
 */
export function isVersionDismissed(
  dismissedVersion: string | null,
  currentVersion: string,
): boolean {
  return dismissedVersion !== null && dismissedVersion === currentVersion;
}

export function useUpdateChecker(): UpdateCheckerResult {
  const isRemote = isRemoteClient();

  const [state, setState] = useState<UpdateState>("idle");
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [canAutoUpdate, setCanAutoUpdate] = useState(false);
  const [dismissed, setDismissedState] = useState(false);
  const [remoteClientsConnected, setRemoteClientsConnected] = useState(false);
  const [updateRequested, setUpdateRequested] = useState(false);

  const updateRef = useRef<Update | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stateRef = useRef(state);
  stateRef.current = state;
  // Version the user dismissed this session. Not persisted — resets on
  // every launch so the update prompt keeps nagging until they update.
  const dismissedVersionRef = useRef<string | null>(null);
  // Last version published to the web-remote bridge, so a paired browser only
  // hears about each version once.
  const publishedVersionRef = useRef<string | null>(null);

  const doCheck = useCallback(async () => {
    try {
      setState("checking");
      const update = await check();
      if (update) {
        updateRef.current = update;
        setUpdateVersion(update.version);
        setDismissedState(
          isVersionDismissed(dismissedVersionRef.current, update.version),
        );
        setState("update-available");
      } else {
        setState("idle");
      }
    } catch (e) {
      console.error("[update-checker] check failed:", e);
      setState("idle");
    }
  }, []);

  // ── Desktop: native updater plugin ─────────────────────────────────
  useEffect(() => {
    if (import.meta.env.DEV) return;
    // The remote client's shim nulls the updater plugin, so the native check
    // machinery must never run there — the web variant below drives it instead.
    if (isRemote) return;

    getPackageFormat()
      .then((fmt) => setCanAutoUpdate(canAutoUpdateFormat(fmt)))
      .catch(() => setCanAutoUpdate(false));

    timeoutRef.current = setTimeout(() => {
      doCheck();
      intervalRef.current = setInterval(doCheck, CHECK_INTERVAL);
    }, INITIAL_DELAY);

    return () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [doCheck, isRemote]);

  const startDownload = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;

    try {
      setState("downloading");
      setDownloadProgress(0);

      let downloaded = 0;
      let contentLength = 0;

      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            contentLength = event.data.contentLength ?? 0;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            if (contentLength > 0) {
              setDownloadProgress(Math.min(100, Math.round((downloaded / contentLength) * 100)));
            }
            break;
          case "Finished":
            setDownloadProgress(100);
            break;
        }
      });

      setState("ready");
    } catch (e) {
      console.error("[update-checker] download failed:", e);
      setState("error");
    }
  }, []);

  const installAndRestart = useCallback(async () => {
    try {
      await relaunch();
    } catch (e) {
      console.error("[update-checker] relaunch failed:", e);
      setState("error");
    }
  }, []);

  const dismiss = useCallback(() => {
    dismissedVersionRef.current = updateVersion;
    setDismissedState(true);
  }, [updateVersion]);

  // ── Desktop: publish availability to paired web clients ────────────
  //
  // Update state lives only in this hook. Push it to the web-remote server so a
  // paired browser (which has no updater plugin) can surface a "desktop update
  // available" prompt, and so late joiners see it via the `web_remote_status`
  // snapshot. Fires at most once per version; a no-op observable-side when no
  // web client is listening.
  useEffect(() => {
    if (isRemote || !updateVersion) return;
    if (publishedVersionRef.current === updateVersion) return;
    publishedVersionRef.current = updateVersion;
    webRemotePublishUpdateAvailable(true, updateVersion).catch(() => {});
  }, [isRemote, updateVersion]);

  // ── Desktop: track attached remote devices (only while an update exists) ──
  useEffect(() => {
    if (isRemote || !updateVersion) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const apply = (status: WebRemoteStatus | null) => {
      if (!cancelled) setRemoteClientsConnected(remoteClientsAttached(status));
    };

    webRemoteStatus().then(apply).catch(() => {});
    onWebRemoteStateChanged(apply)
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [isRemote, updateVersion]);

  // ── Desktop: honor a web client's "update & restart desktop" request ──
  useEffect(() => {
    if (isRemote) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    listen(UPDATE_REQUESTED_EVENT, () => {
      const action = updateAdvanceAction(
        stateRef.current,
        updateRef.current !== null,
      );
      if (action === "none") return;
      // Re-reveal the toast if it was dismissed on the desktop, so the standard
      // download/restart confirmation UX is visible for the web-triggered flow.
      setDismissedState(false);
      if (action === "download") startDownload();
      else installAndRestart();
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [isRemote, startDownload, installAndRestart]);

  // ── Web: learn about desktop updates from the server status ────────
  //
  // No updater plugin here. Availability comes from the `web_remote_status`
  // snapshot on mount (covers a client that pairs after the update was found)
  // plus live `web-remote-state-changed` deltas.
  useEffect(() => {
    if (!isRemote) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    const apply = (status: WebRemoteStatus | null) => {
      if (cancelled) return;
      const { available, version } = desktopUpdateFromStatus(status);
      if (available) {
        setUpdateVersion(version);
        setState("update-available");
      } else {
        setUpdateVersion(null);
        setState("idle");
        setUpdateRequested(false);
      }
    };

    webRemoteStatus().then(apply).catch(() => {});
    onWebRemoteStateChanged(apply)
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [isRemote]);

  const requestDesktopUpdate = useCallback(() => {
    setUpdateRequested(true);
    webRemoteRequestUpdate().catch((e) => {
      console.error("[update-checker] request desktop update failed:", e);
      setUpdateRequested(false);
    });
  }, []);

  // ── Publish to the shared status store ────────────────────────────
  //
  // This hook owns the only poll, so anything else that wants to show update
  // state (the app-menu footer strip) reads the mirror instead of mounting a
  // second checker and doubling the round trips. `isRemote` and
  // `requestDesktopUpdate` ride along because on the web client `startDownload`
  // is a no-op — a reader that acts on the state needs the same remote branch
  // the toast takes.
  const publishUpdateStatus = useUpdateStatusStore((s) => s.publish);
  useEffect(() => {
    publishUpdateStatus({
      state,
      updateVersion,
      downloadProgress,
      isRemote,
      startDownload,
      installAndRestart,
      requestDesktopUpdate,
    });
  }, [
    publishUpdateStatus,
    state,
    updateVersion,
    downloadProgress,
    isRemote,
    startDownload,
    installAndRestart,
    requestDesktopUpdate,
  ]);

  return {
    state,
    updateVersion,
    downloadProgress,
    canAutoUpdate,
    startDownload,
    installAndRestart,
    dismiss,
    dismissed,
    isRemote,
    remoteClientsConnected,
    requestDesktopUpdate,
    updateRequested,
  };
}
