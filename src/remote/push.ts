import { invoke } from "@tauri-apps/api/core";
import { remoteViewHost } from "./client-view";
import { isRemoteClient } from "@/components/remote/is-remote-client";

export type PushCategories = {
  attention: boolean;
  complete: boolean;
  failure: boolean;
};
export const defaultCategories: PushCategories = {
  attention: true,
  complete: true,
  failure: true,
};
export function isStandalone(): boolean {
  return (
    matchMedia("(display-mode: standalone)").matches ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true
  );
}
export function isAppleMobile(): boolean {
  return (
    /iPhone|iPad|iPod/.test(navigator.userAgent) ||
    (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1)
  );
}
export function pushUnavailableReason(): string | null {
  if (!window.isSecureContext)
    return "Open Codemux over HTTPS to install it and enable notifications.";
  if (isAppleMobile() && !isStandalone())
    return "Add Codemux to your Home Screen, then open its icon to enable notifications.";
  if (
    !("serviceWorker" in navigator) ||
    !("PushManager" in window) ||
    !("Notification" in window)
  )
    return "This browser does not support background notifications. Try an up-to-date Safari or Chrome.";
  if (!isRemoteClient())
    return "Connect to your desktop to enable notifications.";
  return null;
}
function scope(): string {
  // Service-worker scopes reject encoded slashes. LAN host identities are
  // origins (https://…), so use a URL-safe base64 path segment instead.
  const bytes = new TextEncoder().encode(remoteViewHost());
  const key = btoa(
    Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""),
  )
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  return `/push/${key}/`;
}
export async function pushRegistration(): Promise<ServiceWorkerRegistration> {
  return navigator.serviceWorker.register("/sw.js", {
    scope: scope(),
    updateViaCache: "none",
  });
}
export async function currentSubscription(): Promise<PushSubscription | null> {
  if (!("serviceWorker" in navigator)) return null;
  return (
    (
      await navigator.serviceWorker.getRegistration(scope())
    )?.pushManager.getSubscription() ?? null
  );
}
export function decodeApplicationKey(value: string): Uint8Array {
  const raw = atob(value.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}
export async function enablePush(categories: PushCategories): Promise<void> {
  const reason = pushUnavailableReason();
  if (reason) throw new Error(reason);
  // This must be the first awaited operation: iOS requires the actual button gesture.
  const permission = await Notification.requestPermission();
  if (permission !== "granted")
    throw new Error(
      permission === "denied"
        ? "Notifications are blocked. Enable them in your device’s notification settings, then return here."
        : "Notifications were not enabled. You can try again whenever you’re ready.",
    );
  const config = await invoke<{ public_key: string }>("web_push_config");
  const registration = await pushRegistration();
  if (!registration.active)
    await new Promise<void>((resolve, reject) => {
      const worker = registration.installing ?? registration.waiting;
      if (!worker) {
        reject(new Error("Notification worker could not start."));
        return;
      }
      const timeout = setTimeout(() => {
        worker.removeEventListener("statechange", changed);
        reject(new Error("Notification worker timed out. Try again."));
      }, 15000);
      const changed = () => {
        if (worker.state === "activated") {
          clearTimeout(timeout);
          worker.removeEventListener("statechange", changed);
          resolve();
        }
      };
      worker.addEventListener("statechange", changed);
      changed();
    });
  let subscription = await registration.pushManager.getSubscription();
  const key = decodeApplicationKey(config.public_key);
  const existingKey = subscription?.options?.applicationServerKey;
  if (
    subscription &&
    existingKey &&
    (existingKey.byteLength !== key.length ||
      new Uint8Array(existingKey).some((byte, i) => byte !== key[i]))
  ) {
    await subscription.unsubscribe();
    subscription = null;
  }
  const created = !subscription;
  subscription ??= await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: key,
  });
  try {
    await invoke("web_push_subscribe", {
      subscription: subscription.toJSON(),
      categories,
      origin: location.origin,
      host: remoteViewHost(),
    });
  } catch (error) {
    if (created) await subscription.unsubscribe().catch(() => false);
    throw error;
  }
}
export async function disablePush(): Promise<void> {
  const subscription = await currentSubscription();
  // Revoke at the host first. Failure must not claim that delivery was disabled.
  await invoke("web_push_unsubscribe");
  await subscription?.unsubscribe();
}
export async function updatePushCategories(
  categories: PushCategories,
): Promise<void> {
  await invoke("web_push_preferences", { categories });
}
