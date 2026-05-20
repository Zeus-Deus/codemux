import { useState, useEffect, useRef, useCallback } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getPackageFormat } from "@/tauri/commands";

type UpdateState =
  | "idle"
  | "checking"
  | "update-available"
  | "downloading"
  | "ready"
  | "error";

interface UpdateCheckerResult {
  state: UpdateState;
  updateVersion: string | null;
  downloadProgress: number;
  canAutoUpdate: boolean;
  startDownload: () => void;
  installAndRestart: () => void;
  dismiss: () => void;
  dismissed: boolean;
}

const CHECK_INTERVAL = 4 * 60 * 60 * 1000; // 4 hours
const INITIAL_DELAY = 5000; // 5 seconds

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
  const [state, setState] = useState<UpdateState>("idle");
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [canAutoUpdate, setCanAutoUpdate] = useState(false);
  const [dismissed, setDismissedState] = useState(false);

  const updateRef = useRef<Update | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Version the user dismissed this session. Not persisted — resets on
  // every launch so the update prompt keeps nagging until they update.
  const dismissedVersionRef = useRef<string | null>(null);

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

  useEffect(() => {
    if (import.meta.env.DEV) return;

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
  }, [doCheck]);

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

  return {
    state,
    updateVersion,
    downloadProgress,
    canAutoUpdate,
    startDownload,
    installAndRestart,
    dismiss,
    dismissed,
  };
}
