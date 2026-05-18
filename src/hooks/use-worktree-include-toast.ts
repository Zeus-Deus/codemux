import { useEffect } from "react";
import { onWorktreeIncludesApplied } from "@/tauri/events";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";

/**
 * Shows a one-time info toast when worktree includes are applied from defaults,
 * so users learn about the feature and how to customize it.
 *
 * Implementation note: do NOT subscribe to `appState` via `useAppStore`
 * here, and do NOT put `appState` in the effect's dependency array.
 *
 * Why: this hook is mounted once at the app shell. If we subscribed to
 * `appState`, the hook would re-run on every backend `app-state-changed`
 * tick (every agent token, every git poll, every hook event — many
 * times per second under load). That alone is cheap, but the
 * `[appState]` dep array would also tear down + re-register the Tauri
 * event listener on every tick, which is a real cost (each register /
 * unregister is an IPC round-trip). The original code accidentally
 * leaked listener registrations and added perceptible workspace-switch
 * latency by performing dozens of attach/detach cycles per second
 * during the activate burst.
 *
 * Instead, register the listener exactly once. When the worktree-
 * includes-applied event fires, look up the workspace via
 * `useAppStore.getState()` at event time — events are rare and the
 * lookup is O(workspaces).
 */
export function useWorktreeIncludeToast() {
  useEffect(() => {
    const unlisten = onWorktreeIncludesApplied(async (payload) => {
      // Only show hint for defaults — if user configured a file or setting, they already know
      if (payload.source !== "defaults" || payload.copied.length === 0) {
        return;
      }

      // Find the workspace to get its project root. Read via getState
      // at event time so we don't subscribe to appState above.
      const appState = useAppStore.getState().appState;
      const ws = appState?.workspaces.find(
        (w) => w.workspace_id === payload.workspace_id,
      );
      const projectRoot = ws?.project_root ?? ws?.cwd;
      if (!projectRoot) return;

      const key = `worktree-include-hint-shown:${projectRoot}`;
      const shown = await dbGetUiState(key).catch(() => null);
      if (shown === "true") return;

      const fileNames = payload.copied.map((f) => basename(f)).join(", ");
      toast.info(
        `Copied default files (${fileNames}) to worktree. Customize in Settings > Projects or add a .codemuxinclude file.`,
        { duration: 8000 },
      );

      await dbSetUiState(key, "true").catch(console.error);
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);
}
