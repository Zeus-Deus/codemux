import { isRemoteClient } from "@/components/remote/is-remote-client";

/** Release a browser channel whose consumer has gone away. Native Tauri
 *  channels release themselves when the backend sends their end marker. */
export function releaseRemoteChannel(id: number): void {
  if (!isRemoteClient()) return;
  const { __TAURI_INTERNALS__ } = window as Window & {
    __TAURI_INTERNALS__?: { unregisterCallback(id: number): void };
  };
  __TAURI_INTERNALS__?.unregisterCallback(id);
}
