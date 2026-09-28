import { Channel, invoke } from "@tauri-apps/api/core";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { clearAddonContext, useAddonsStore } from "@/stores/addons-store";
import type { AddonEvent, AddonInventory, AddonMount } from "./types";
import { addonError } from "./types";
export function addonInvoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (isRemoteClient())
    return Promise.reject(
      addonError("REMOTE_UNSUPPORTED", "Add-ons run only in the desktop app"),
    );
  return invoke<T>(command, args);
}
export const addonInventory = () =>
  addonInvoke<AddonInventory>("addon_inventory");
/** The app's event receiver, and whether its stream is currently open. */
let receiver: ((event: AddonEvent) => void) | null = null;
let subscribed = false;
export async function subscribeAddons(
  receive: (event: AddonEvent) => void,
): Promise<void> {
  receiver = receive;
  subscribed = false;
  const channel = new Channel<AddonEvent>();
  channel.onmessage = receive;
  await addonInvoke<void>("addon_subscribe", { channel });
  if (receiver === receive) subscribed = true;
}
/**
 * Open the event stream again after the add-on manager itself was reopened
 * (a registry reset or a retried open). The first subscription failed with
 * the manager, so nothing would reach the app. Subscribing resets the host's
 * workspace, so bind the current one again before accepting views.
 */
export async function resubscribeAddons(
  workspace: () => string | null,
): Promise<void> {
  if (!receiver || subscribed) return;
  await subscribeAddons(receiver);
  clearAddonContext();
  const revision = useAddonsStore.getState().contextRevision;
  await addonInvoke("addon_context_changed", {
    workspaceId: workspace(),
  }).catch(() => {});
  if (revision === useAddonsStore.getState().contextRevision)
    useAddonsStore.setState({ ready: true });
}
export const mountAddon = (
  id: string,
  view: string,
  kind: "panels" | "composerViews",
  workspaceId: string,
  composerId: string | null,
) =>
  addonInvoke<AddonMount>("addon_mount", {
    id,
    view,
    kind,
    workspaceId,
    composerId,
  });
