import { toast as sonnerToast, type ExternalToast } from "sonner";

const DURATION = {
  info: 5000,
  success: 4000,
  warning: 6000,
  error: 8000,
} as const;

type ToastLevel = keyof typeof DURATION;

function fire(level: ToastLevel, message: string, opts?: ExternalToast) {
  return sonnerToast[level](message, { duration: DURATION[level], ...opts });
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
// - We deliberately do NOT show a follow-up undo on the undo's own
//   success toast. The reverse-action's outcome surfaces as a plain
//   success/error toast.
// - The default 10s window matches the standard "send-undo" UX
//   pattern. Callers can override per-action if a different window
//   makes sense (e.g. multi-GB pushes might want 30s so the network
//   round-trip catches up).
const UNDO_DURATION_MS = 10_000;

export interface UndoableOptions {
  /** The toast headline, e.g. `"Pushed to homedesk"`. */
  message: string;
  /** Optional secondary line. */
  description?: string;
  /** Label for the undo action button. Default: "Undo". */
  undoLabel?: string;
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
  return sonnerToast.success(opts.message, {
    duration: dur,
    description: opts.description,
    action: {
      label: opts.undoLabel ?? "Undo",
      onClick: () => {
        if (inProgress) return;
        inProgress = true;
        opts
          .onUndo()
          .then(() => {
            // Reverse-action's own success surfaces via a plain
            // success toast — keep the messaging up to the caller's
            // onUndo so it can describe what actually happened.
          })
          .catch((err) => {
            const message = err instanceof Error ? err.message : String(err);
            sonnerToast.error("Undo failed", {
              description: message,
              duration: DURATION.error,
            });
          });
      },
    },
  });
}

export const toast = {
  info: (msg: string, opts?: ExternalToast) => fire("info", msg, opts),
  success: (msg: string, opts?: ExternalToast) => fire("success", msg, opts),
  warning: (msg: string, opts?: ExternalToast) => fire("warning", msg, opts),
  error: (msg: string, opts?: ExternalToast) => fire("error", msg, opts),
  dismiss: sonnerToast.dismiss,
  /** Raw sonner toast for custom/persistent toasts (e.g. update prompt). */
  custom: sonnerToast,
  /** Success toast with a reverse-action button. See `fireUndoable`. */
  undoable: fireUndoable,
};
