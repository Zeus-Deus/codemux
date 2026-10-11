import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  openNotificationTarget,
  type NotificationTargetIds,
} from "@/lib/open-notification-target";

/** Must match `NOTIFICATION_ACTIVATE_EVENT` in `src-tauri/src/notifications.rs`. */
export const NOTIFICATION_ACTIVATE_EVENT = "notification-activate";

/**
 * Desktop only: when the user clicks a native agent notification, the backend
 * raises the window and emits this event so we can open the agent's pane.
 * Remote clients open targets from their own toast / Web Notification.
 */
export function useNotificationActivate(): void {
  useEffect(() => {
    if (isRemoteClient()) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<NotificationTargetIds>(NOTIFICATION_ACTIVATE_EVENT, (event) => {
      void openNotificationTarget(event.payload);
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
