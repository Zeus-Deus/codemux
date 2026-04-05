import { useEffect } from "react";
import { onWorktreeIncludesApplied } from "@/tauri/events";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { toast } from "@/lib/toast";

/**
 * Shows a one-time info toast when worktree includes are applied from defaults,
 * so users learn about the feature and how to customize it.
 */
export function useWorktreeIncludeToast() {
  const appState = useAppStore((s) => s.appState);

  console.log("HOOK MOUNTED");

  useEffect(() => {
    const unlisten = onWorktreeIncludesApplied(async (payload) => {
      console.log("EVENT RECEIVED", payload);

      // Only show hint for defaults — if user configured a file or setting, they already know
      if (payload.source !== "defaults" || payload.copied.length === 0) {
        console.log("SKIPPED — source:", payload.source, "copied:", payload.copied.length);
        return;
      }

      // Find the workspace to get its project root
      const ws = appState?.workspaces.find(
        (w) => w.workspace_id === payload.workspace_id,
      );
      const projectRoot = ws?.project_root ?? ws?.cwd;
      if (!projectRoot) {
        console.log("SKIPPED — no projectRoot found for workspace", payload.workspace_id);
        return;
      }

      const key = `worktree-include-hint-shown:${projectRoot}`;
      const shown = await dbGetUiState(key).catch(() => null);
      if (shown === "true") {
        console.log("SKIPPED — already shown for", projectRoot);
        return;
      }

      console.log("TOAST FIRED");
      const fileNames = payload.copied.map((f) => f.split("/").pop()).join(", ");
      toast.info(
        `Copied default files (${fileNames}) to worktree. Customize in Settings > Projects or add a .codemuxinclude file.`,
        { duration: 8000 },
      );

      await dbSetUiState(key, "true").catch(console.error);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appState]);
}
