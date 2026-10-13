import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isRemoteClient } from "@/components/remote/is-remote-client";

/**
 * Window-wide drop target for a folder from the OS file manager, for the
 * full-screen Open Project page. Uses the webview's native drag-drop event
 * because that is the only one that carries real filesystem paths.
 *
 * Returns whether something is being dragged over the window, so the page
 * can show where the drop will land. Only the first dropped path is used;
 * the caller decides whether it is a folder it can open.
 */
export function useFolderDrop(onDrop: (path: string) => void): boolean {
  const [dragging, setDragging] = useState(false);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;

  useEffect(() => {
    // A web remote client drives a browser tab with no local filesystem.
    if (isRemoteClient()) return;
    let unlisten: (() => void) | null = null;
    let disposed = false;
    try {
      getCurrentWebview()
        .onDragDropEvent(({ payload }) => {
          if (payload.type === "enter" || payload.type === "over") {
            setDragging(true);
          } else if (payload.type === "leave") {
            setDragging(false);
          } else if (payload.type === "drop") {
            setDragging(false);
            const [path] = payload.paths;
            if (path) onDropRef.current(path);
          }
        })
        .then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        })
        .catch(() => {});
    } catch {
      // Outside a Tauri webview (tests, plain browser) there is nothing to
      // listen to; the page still works through its buttons.
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return dragging;
}
