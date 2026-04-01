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

function isDismissed(version: string): boolean {
  try {
    return localStorage.getItem(`codemux-update-dismissed-${version}`) === "true";
  } catch {
    return false;
  }
}

function setDismissed(version: string) {
  try {
    localStorage.setItem(`codemux-update-dismissed-${version}`, "true");
  } catch {
    // localStorage unavailable
  }
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

  const doCheck = useCallback(async () => {
    try {
      setState("checking");
      const update = await check();
      if (update) {
        updateRef.current = update;
        setUpdateVersion(update.version);
        setDismissedState(isDismissed(update.version));
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
      .then((fmt) => setCanAutoUpdate(fmt === "appimage"))
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
    if (updateVersion) {
      setDismissed(updateVersion);
    }
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
