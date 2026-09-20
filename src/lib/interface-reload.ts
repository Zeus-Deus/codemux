/**
 * Recovery reload for the interface — the UI half of `webview_recovery.rs`.
 *
 * The React UI runs in a renderer process of its own. When that process dies
 * or wedges, the window goes blank or stops responding while the backend, the
 * PTY daemon, agents and every running command carry on outside it. Reloading
 * *only the page* brings the UI back: panes re-attach to their live sessions
 * and replay from the backend. Nothing here restarts Codemux.
 *
 * Two doors, one flow:
 *
 * - **Ctrl+Shift+R** (`reloadInterface`) and the command palette's "Reload
 *   interface" row both call {@link requestInterfaceReload}, which asks for
 *   confirmation first so a browser-habit keypress cannot throw away what the
 *   user was typing.
 * - On Linux the chord never reaches the page: the app process grabs it on the
 *   GTK toplevel — that is what makes it work when the renderer is dead — and
 *   emits {@link RELOAD_REQUESTED_EVENT} instead. {@link useInterfaceReloadRequests}
 *   answers that event with `ack_reload_request` ("still alive, I'll ask the
 *   user") and opens the same dialog. A page that never answers is wedged, and
 *   the app process reloads it without asking.
 *
 * So a *second* chord press while the dialog is open means "stop asking" and
 * reloads immediately — the escape hatch for a page that is live enough to run
 * this code but not to paint the dialog.
 */
import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

import { isRemoteClient } from "@/components/remote/is-remote-client";

/** Emitted by the app process when the recovery chord is pressed. */
export const RELOAD_REQUESTED_EVENT = "codemux://reload-requested";

interface InterfaceReloadState {
  /** True while the confirmation dialog is up. */
  prompting: boolean;
  setPrompting: (prompting: boolean) => void;
}

export const useInterfaceReloadStore = create<InterfaceReloadState>((set) => ({
  prompting: false,
  setPrompting: (prompting) => set({ prompting }),
}));

/**
 * Ask to reload the interface. Opens the confirmation dialog; if it is already
 * open, the user has asked twice and gets the reload straight away.
 */
export function requestInterfaceReload(): void {
  const { prompting } = useInterfaceReloadStore.getState();
  if (prompting) {
    reloadInterfaceNow();
    return;
  }
  useInterfaceReloadStore.getState().setPrompting(true);
}

/** Close the dialog without reloading. */
export function cancelInterfaceReload(): void {
  useInterfaceReloadStore.getState().setPrompting(false);
}

/**
 * Reload the page, and only the page. The app process performs the navigation
 * (`reload_main_window` → `WebviewWindow::reload()`), so backend state, PTY
 * sessions, sidecars and agents are untouched.
 *
 * In the web remote client there is no desktop webview to recover — the "page"
 * is the user's own browser tab — so it reloads that instead. The desktop's
 * webview is deliberately never reloaded from a remote client.
 */
export function reloadInterfaceNow(): void {
  useInterfaceReloadStore.getState().setPrompting(false);
  if (isRemoteClient()) {
    window.location.reload();
    return;
  }
  void invoke<void>("reload_main_window").catch((err) => {
    console.error("[interface-reload] reload_main_window failed:", err);
  });
}

/**
 * Listen for the app process asking the page to reload (Linux recovery chord).
 * Answering keeps the flow in the page's hands: the user gets the dialog
 * instead of losing the UI under them. Desktop only — a remote browser tab is
 * not the webview the app process is trying to recover.
 */
export function useInterfaceReloadRequests(): void {
  useEffect(() => {
    if (isRemoteClient()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    try {
      void listen<{ token: number }>(RELOAD_REQUESTED_EVENT, (event) => {
        // Answer first, then ask: the app process only waits about a second
        // before deciding the page is wedged and reloading it anyway.
        void invoke<void>("ack_reload_request", {
          token: event.payload.token,
        }).catch(() => {});
        requestInterfaceReload();
      }).then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    } catch {
      // `listen()` throws synchronously outside a Tauri webview (tests, the
      // dev mock). There is no native reload to hear about there.
    }
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
