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

export const toast = {
  info: (msg: string, opts?: ExternalToast) => fire("info", msg, opts),
  success: (msg: string, opts?: ExternalToast) => fire("success", msg, opts),
  warning: (msg: string, opts?: ExternalToast) => fire("warning", msg, opts),
  error: (msg: string, opts?: ExternalToast) => fire("error", msg, opts),
  dismiss: sonnerToast.dismiss,
  /** Raw sonner toast for custom/persistent toasts (e.g. update prompt). */
  custom: sonnerToast,
};
