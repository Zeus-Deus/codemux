/**
 * Typed helper for the `web-remote-state-changed` event.
 *
 * The web-remote server (`src-tauri/src/web_remote/`) emits this on the
 * global bus every time the server or the paired-device set changes, so
 * both the desktop settings panel and any connected web client can live-
 * update. The payload is the full `WebRemoteStatus` snapshot.
 *
 * This lives under `src/remote/` (not `src/tauri/events.ts`) because the
 * events module is owned by another lane; keeping the helper here avoids
 * cross-lane edits while matching the same `on<Event>` convention.
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { WebRemoteStatus } from "@/tauri/types";

/** Global event name — must match `STATE_CHANGED_EVENT` in
 *  `src-tauri/src/web_remote/mod.rs`. */
export const WEB_REMOTE_STATE_CHANGED_EVENT = "web-remote-state-changed";

/** Subscribe to server/device state changes. Resolves to an unlisten fn. */
export const onWebRemoteStateChanged = (
  cb: (status: WebRemoteStatus) => void,
): Promise<UnlistenFn> =>
  listen<WebRemoteStatus>(WEB_REMOTE_STATE_CHANGED_EVENT, (e) => cb(e.payload));
