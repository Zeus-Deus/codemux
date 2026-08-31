import { useCallback } from "react";

import { toast } from "@/lib/toast";
import {
  onActiveWorkspacePersistFailed,
  type ActiveWorkspacePersistFailure,
} from "@/tauri/events";
import { useTauriEvent } from "./use-tauri-event";

/**
 * Surface the rare durability failure from the non-blocking workspace
 * selection writer. The raw SQLite error remains in the console for support,
 * while the toast stays path/identifier-free.
 */
export function useActiveWorkspacePersistenceErrors(): void {
  const handleFailure = useCallback((failure: ActiveWorkspacePersistFailure) => {
    console.error(
      `[workspace-persistence] generation ${failure.generation} failed:`,
      failure.error,
    );
    toast.error("Couldn't save the active workspace", {
      description:
        "Your current workspace is still open, but CodeMux may restore the previous selection after restart.",
    });
  }, []);

  useTauriEvent(onActiveWorkspacePersistFailed, handleFailure, [handleFailure]);
}
