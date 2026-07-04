import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, Copy, X } from "lucide-react";
import { isRemoteClient } from "@/components/remote/is-remote-client";

/**
 * Standalone window controls (minimize / maximize / close) used by full-
 * screen views that render OUTSIDE the regular `<TitleBar />`.
 *
 * Codemux runs with `decorations: false` in `tauri.conf.json` so the OS
 * never paints native chrome. The main app shell embeds these controls in
 * `title-bar.tsx`, but the login screen, empty state, settings view, and
 * new-project screen all short-circuit `AppShell` BEFORE the title bar
 * mounts — on Windows that left those screens with literally no way to
 * close, minimize, or maximize the app. (On Linux + Hyprland the WM
 * decorates the window itself even with `decorations: false`, which is
 * why we never noticed.)
 *
 * `<WindowControls />` is the same button cluster as in `title-bar.tsx`
 * extracted so it can be reused without dragging in the rest of the title
 * bar's content. `<WindowChrome />` wraps it in a draggable strip that
 * spans the full window width so the user can drag the entire top edge to
 * move the window — matching the chrome behavior they get inside the
 * regular title bar.
 *
 * Both components are platform-neutral: on Linux they're harmless extras
 * (the Hyprland WM controls still work), on Windows they're the only
 * way to manipulate the window.
 */
export function WindowControls() {
  // The web remote client drives a browser tab, not the desktop OS window —
  // minimize/maximize/close would target the wrong (or no) window. Render
  // nothing there and skip the window-plugin IPC entirely. Desktop is
  // unchanged: `remote` is always false inside the real Tauri webview.
  const remote = isRemoteClient();
  const [isMaximized, setIsMaximized] = useState(false);
  const appWindow = getCurrentWindow();

  useEffect(() => {
    if (remote) return;
    appWindow.isMaximized().then(setIsMaximized);
    const unlisten = appWindow.onResized(() => {
      appWindow.isMaximized().then(setIsMaximized);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appWindow, remote]);

  if (remote) return null;

  return (
    <div className="flex items-center">
      <button
        type="button"
        aria-label="Minimize"
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
        onClick={() => appWindow.minimize()}
      >
        <Minus className="h-3 w-3" />
      </button>
      <button
        type="button"
        aria-label={isMaximized ? "Restore" : "Maximize"}
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
        onClick={() => appWindow.toggleMaximize()}
      >
        {isMaximized ? (
          <Copy className="h-3 w-3" />
        ) : (
          <Square className="h-2.5 w-2.5" />
        )}
      </button>
      <button
        type="button"
        aria-label="Close"
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-destructive hover:text-destructive-foreground"
        onClick={() => appWindow.close()}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

/**
 * Top-of-window draggable strip with `<WindowControls />` anchored to the
 * right. Use this from any full-screen view that doesn't render
 * `<TitleBar />` so the user keeps a way to move/minimize/maximize/close
 * the window.
 *
 * The strip is rendered absolutely positioned at the top of its parent so
 * it overlays the existing layout without forcing every screen to budget
 * vertical space for it. The drag region covers the full width minus the
 * controls cluster on the right. The whole strip is `pointer-events-none`
 * by default so it doesn't intercept clicks on content beneath it; the
 * drag region and the controls each re-enable pointer events on themselves.
 */
export function WindowChrome() {
  // On the web remote client there is no OS window to drag or control, so the
  // whole strip (drag region + controls) is inert. Render nothing — callers
  // reserve the top spacing (`pt-7`) themselves, so layout stays stable and
  // the strip's `data-tauri-drag-region` overlay never becomes a click dead
  // zone in the browser. Desktop behavior is unchanged.
  if (isRemoteClient()) return null;

  return (
    <div className="pointer-events-none absolute inset-x-0 top-0 z-50 flex h-7 items-center justify-end">
      <div
        data-tauri-drag-region
        className="pointer-events-auto absolute inset-0"
      />
      <div className="pointer-events-auto relative">
        <WindowControls />
      </div>
    </div>
  );
}
