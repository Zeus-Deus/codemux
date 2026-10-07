import type { CSSProperties } from "react";
import { toast as sonnerToast, type ExternalToast } from "sonner";
import { copyToClipboard, COPY_FAILED_MESSAGE } from "@/lib/clipboard";

const DURATION = {
  info: 5000,
  success: 4000,
  warning: 6000,
  error: 8000,
} as const;

type ToastLevel = keyof typeof DURATION;

function fire(level: ToastLevel, message: string, opts?: ExternalToast) {
  return sonnerToast[level](message, {
    duration: DURATION[level],
    // An error is the toast someone wants gone before its timer, or wants to
    // keep reading past it; a visible close beats an undiscoverable swipe.
    closeButton: level === "error",
    ...opts,
  });
}

/**
 * The readable part of a rejection. Tauri commands reject with plain strings
 * and JS throws `Error`s; either way the "Error: " prefix that `String(err)`
 * adds is noise in a toast.
 */
export function errorMessage(err: unknown): string {
  const raw =
    typeof err === "string"
      ? err
      : err instanceof Error
        ? err.message
        : typeof err === "object" && err !== null && "message" in err && typeof err.message === "string"
          ? err.message
          : String(err);
  return raw.replace(/^(?:Uncaught\s+)?[A-Za-z]*Error:\s*/, "").trim() || "Unknown error";
}

/**
 * The one shape for an error toast: a short title saying what failed, the
 * cleaned error as a clamped description, and a "Copy details" action so a
 * long git or IPC message can be pasted somewhere instead of retyped.
 */
function fireFailure(title: string, err: unknown, opts?: ExternalToast) {
  const detail = errorMessage(err);
  return fire("error", title, {
    description: detail,
    classNames: { description: "line-clamp-3 break-words" },
    action: {
      label: "Copy details",
      onClick: (event) => {
        // Copying is not the same as being done with the toast.
        event.preventDefault();
        void copyToClipboard(detail).then((copied) => {
          if (!copied) fire("error", COPY_FAILED_MESSAGE);
        });
      },
    },
    ...opts,
  });
}

/**
 * A `.catch` handler for user-triggered actions (shortcuts, palette rows,
 * toolbar buttons) that would otherwise fail with only a console line.
 */
export function reportFailure(title: string): (err: unknown) => void {
  return (err) => {
    console.error(`${title}:`, err);
    fireFailure(title, err);
  };
}

// ── Undoable actions ────────────────────────────────────────────
//
// An "undoable" success toast holds a reverse-action closure that
// the user can invoke for ~10 seconds. Used by push/pull/adopt so
// every state-changing action has a one-click escape hatch.
//
// Design notes:
// - The undo closure runs ONCE — `inProgress` guards against double-
//   clicks (e.g. user double-taps the Undo button while the reverse
//   action is still running).
// - Clicking Undo keeps the toast and turns it into a spinner while the
//   reverse action runs (it can be a multi-second transfer), so the click
//   is visibly acknowledged. The reverse action's outcome then surfaces
//   as the caller's own success/error toast, or "Undo failed" in place.
// - The default 10s window matches the standard "send-undo" UX
//   pattern. Callers can override per-action if a different window
//   makes sense (e.g. multi-GB pushes might want 30s so the network
//   round-trip catches up). A bar along the toast's bottom edge drains
//   over that window (globals.css, `.cn-toast-undo`).
const UNDO_DURATION_MS = 10_000;

export interface UndoableOptions {
  /** The toast headline, e.g. `"Pushed to homedesk"`. */
  message: string;
  /** Optional secondary line. */
  description?: string;
  /** Label for the undo action button. Default: "Undo". */
  undoLabel?: string;
  /** Headline while the reverse action runs. Default: "Undoing…". */
  pendingLabel?: string;
  /** Async closure that reverses the just-completed action. Runs
   *  exactly once; subsequent clicks are guarded against. */
  onUndo: () => Promise<void>;
  /** Override the 10s default if the action is unusually slow or
   *  fast. Clamped to [3000, 60000]. */
  durationMs?: number;
}

export function fireUndoable(opts: UndoableOptions): string | number {
  const dur = Math.min(
    Math.max(opts.durationMs ?? UNDO_DURATION_MS, 3_000),
    60_000,
  );
  let inProgress = false;
  const id = sonnerToast.success(opts.message, {
    duration: dur,
    description: opts.description,
    className: "cn-toast-undo",
    style: { "--toast-undo-duration": `${dur}ms` } as CSSProperties,
    action: {
      label: opts.undoLabel ?? "Undo",
      onClick: (event) => {
        // Without this Sonner removes the toast the moment Undo is clicked.
        event.preventDefault();
        if (inProgress) return;
        inProgress = true;
        sonnerToast.loading(opts.pendingLabel ?? "Undoing…", {
          id,
          description: undefined,
          action: undefined,
          className: undefined,
        });
        opts
          .onUndo()
          .then(() => {
            // The caller's onUndo reports what actually happened.
            sonnerToast.dismiss(id);
          })
          .catch((err) => {
            sonnerToast.error("Undo failed", {
              id,
              description: errorMessage(err),
              duration: DURATION.error,
              closeButton: true,
            });
          });
      },
    },
  });
  return id;
}

export const toast = {
  info: (msg: string, opts?: ExternalToast) => fire("info", msg, opts),
  success: (msg: string, opts?: ExternalToast) => fire("success", msg, opts),
  warning: (msg: string, opts?: ExternalToast) => fire("warning", msg, opts),
  error: (msg: string, opts?: ExternalToast) => fire("error", msg, opts),
  /** Error toast for a caught error: `title` says what failed, the error
   *  becomes a clamped, copyable description. Prefer this to passing a
   *  stringified error as the title. */
  failure: fireFailure,
  dismiss: sonnerToast.dismiss,
  /** Raw sonner toast for custom/persistent toasts (e.g. update prompt). */
  custom: sonnerToast,
  /** Success toast with a reverse-action button. See `fireUndoable`. */
  undoable: fireUndoable,
};
