import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  GIT_CLONE_PROGRESS_EVENT,
  type GitCloneProgress,
} from "@/tauri/events";

/**
 * True when a `git-clone-progress` payload's `targetDir` refers to the
 * clone this UI started with `targetDir`.
 *
 * The frontend routinely hands git a `~`-relative path (the New Project
 * Location placeholder is `~/Projects`), but the backend runs
 * `expand_tilde` before echoing the dir back in the event — so
 * `~/projects/repo` comes back as `/home/user/projects/repo`. A plain
 * equality/endsWith check on the tilde form never matches (the `~` is
 * gone from the expanded path), which would silently filter out every
 * event. Normalize the tilde to its `/...` suffix before comparing.
 *
 * Exported for unit tests.
 */
export function matchesCloneTarget(
  payloadDir: string,
  targetDir: string,
): boolean {
  if (!targetDir) return true;
  if (payloadDir === targetDir) return true;
  if (targetDir.startsWith("~/")) {
    // `~/projects/repo` → match on the `/projects/repo` suffix of the
    // backend's expanded absolute path.
    return payloadDir.endsWith(targetDir.slice(1));
  }
  return payloadDir.endsWith(targetDir);
}

/**
 * Live `git clone --progress` state for one in-flight clone.
 *
 * Subscribes to the `git-clone-progress` Tauri event while `active` is
 * `true`, keeping only updates whose `targetDir` matches `targetDir` (so
 * two concurrent clones don't cross-talk) — see `matchesCloneTarget` for
 * the tilde-expansion wrinkle. Resets to `null` whenever the clone ends
 * (`active` flips false) or on unmount.
 */
export function useCloneProgress(
  active: boolean,
  targetDir: string,
): GitCloneProgress | null {
  const [progress, setProgress] = useState<GitCloneProgress | null>(null);

  useEffect(() => {
    if (!active) {
      setProgress(null);
      return;
    }

    let cancelled = false;
    let unlisten: (() => void) | undefined;

    // `listen()` throws synchronously in jsdom test envs without the Tauri
    // shim — degrade to a static (spinner-only) state instead of crashing.
    try {
      void listen<GitCloneProgress>(GIT_CLONE_PROGRESS_EVENT, (event) => {
        if (cancelled) return;
        const p = event.payload;
        if (!matchesCloneTarget(p.targetDir, targetDir)) return;
        setProgress(p);
      })
        .then((dispose) => {
          if (cancelled) {
            dispose();
            return;
          }
          unlisten = dispose;
        })
        .catch((err) => {
          console.warn("[clone] progress listen failed:", err);
        });
    } catch (err) {
      console.warn("[clone] progress listen unavailable:", err);
    }

    return () => {
      cancelled = true;
      unlisten?.();
      setProgress(null);
    };
  }, [active, targetDir]);

  return progress;
}
