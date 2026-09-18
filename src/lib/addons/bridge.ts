import { Channel, invoke } from "@tauri-apps/api/core";
import { isRemoteClient } from "@/components/remote/is-remote-client";
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
export async function subscribeAddons(
  receive: (event: AddonEvent) => void,
): Promise<void> {
  const channel = new Channel<AddonEvent>();
  channel.onmessage = receive;
  await addonInvoke<void>("addon_subscribe", { channel });
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
