import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { toast } from "@/lib/toast";
import { isRemoteClient } from "@/components/remote/is-remote-client";

/**
 * Web-remote bridge for desktop notifications.
 *
 * The backend (`src-tauri/src/notifications.rs`) emits a global `notification`
 * event alongside every native desktop notification. On the desktop that
 * event is ignored — the OS notification already fired. On the web remote
 * client this hook picks it up and re-raises it in the browser: a real OS
 * notification via the Web Notifications API when the user has granted
 * permission and the tab is hidden, otherwise an in-app toast.
 *
 * Desktop stays byte-identical: the whole hook is gated on the remote flag,
 * so nothing subscribes and no notification logic runs in the Tauri webview.
 */

/** Event name — must match `NOTIFICATION_EVENT` in
 *  `src-tauri/src/notifications.rs`. */
export const NOTIFICATION_EVENT = "notification";

/** Snake_case payload from `NotificationPayload` in
 *  `src-tauri/src/notifications.rs`. */
export interface WebNotificationPayload {
  title: string;
  body: string;
  workspace_title: string;
}

export type NotificationDelivery = "web" | "toast";

/**
 * Decide how to surface a notification on the web remote client. Pure so the
 * matrix (permission × API availability × tab visibility) is unit-testable.
 *
 * - No Web Notifications API (insecure origin, old browser) → always toast.
 * - Permission granted AND the tab is hidden → a real OS notification (the
 *   user isn't looking, so a system-level nudge is the useful signal).
 * - Otherwise (permission denied/default, or the tab is already visible) →
 *   an in-app toast.
 */
export function chooseNotificationDelivery(input: {
  apiAvailable: boolean;
  permission: NotificationPermission | null;
  pageHidden: boolean;
}): NotificationDelivery {
  if (!input.apiAvailable) return "toast";
  if (input.permission === "granted" && input.pageHidden) return "web";
  return "toast";
}

function webNotificationsAvailable(): boolean {
  return typeof window !== "undefined" && "Notification" in window;
}

function showToast(payload: WebNotificationPayload): void {
  toast.info(payload.title, { description: payload.body });
}

function showWebNotification(payload: WebNotificationPayload): void {
  try {
    const notification = new Notification(payload.title, {
      body: payload.body,
    });
    notification.onclick = () => {
      window.focus();
      notification.close();
    };
  } catch {
    // Some engines throw if the API exists but construction is disallowed
    // (e.g. permission revoked between check and use) — fall back to a toast.
    showToast(payload);
  }
}

export function useWebNotifications(): void {
  useEffect(() => {
    if (!isRemoteClient()) return;

    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    // Ask for permission lazily — only the first time an event actually
    // arrives, never on mount, so a paired browser that never gets a
    // notification is never nagged.
    let permissionRequested = false;

    const handle = async (payload: WebNotificationPayload) => {
      const apiAvailable = webNotificationsAvailable();

      if (
        apiAvailable &&
        Notification.permission === "default" &&
        !permissionRequested
      ) {
        permissionRequested = true;
        try {
          await Notification.requestPermission();
        } catch {
          // requestPermission can reject on insecure origins — treat as
          // "not granted" and fall through to the toast path.
        }
      }

      const permission = apiAvailable ? Notification.permission : null;
      const pageHidden =
        typeof document !== "undefined" && document.hidden === true;

      const delivery = chooseNotificationDelivery({
        apiAvailable,
        permission,
        pageHidden,
      });

      if (delivery === "web") {
        showWebNotification(payload);
      } else {
        showToast(payload);
      }
    };

    listen<WebNotificationPayload>(NOTIFICATION_EVENT, (event) => {
      void handle(event.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
