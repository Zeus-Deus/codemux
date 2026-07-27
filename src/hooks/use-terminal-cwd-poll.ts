/**
 * Poll `/proc/<pid>/cwd` for terminal sessions whose shell doesn't report
 * OSC 7, so the pane header's cwd hint works without shell integration.
 *
 * Mounted once at the app root — a single interval and a single batched IPC
 * call covers every pane, rather than one timer per terminal.
 *
 * The request set is deliberately narrow. Each tick asks only about:
 *
 * - the **active workspace's** sessions, since `workspace-main.tsx` renders
 *   only that workspace's pane tree — a background workspace's header isn't
 *   on screen, and its cwd is re-polled within one interval of being shown;
 * - sessions **not** already reporting via OSC 7, which are strictly better
 *   served by the push path. A fully shell-integrated setup therefore sends
 *   an empty set and the tick short-circuits before touching IPC at all.
 *
 * Polling also stops entirely while the window is hidden — a background
 * window has no visible header to update.
 */

import { useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { useTerminalCwdStore } from "@/stores/terminal-cwd-store";
import { terminalSessionCwds } from "@/tauri/commands";
import type { PaneNodeSnapshot } from "@/tauri/types";

/** How often to re-read the shell's directory. Fast enough that a `cd`
 *  feels reflected in the header, slow enough that idle panes cost
 *  essentially nothing (a readlink per session, only while visible). */
const POLL_INTERVAL_MS = 2000;

/** Collect every terminal session id in a pane tree. */
function collectSessionIds(node: PaneNodeSnapshot, out: string[]): void {
  if (node.kind === "terminal") {
    out.push(node.session_id);
    return;
  }
  if (node.kind === "split") {
    for (const child of node.children) collectSessionIds(child, out);
  }
}

export function useTerminalCwdPoll(): void {
  useEffect(() => {
    let cancelled = false;
    let inFlight = false;

    const tick = async () => {
      // Never stack requests: a slow IPC round-trip must not queue up a
      // backlog of polls that all land at once.
      if (inFlight || document.hidden) return;

      const { appState } = useAppStore.getState();
      if (!appState) return;

      const workspace = appState.workspaces.find(
        (w) => w.workspace_id === appState.active_workspace_id,
      );
      if (!workspace) return;

      const visibleIds: string[] = [];
      for (const surface of workspace.surfaces) {
        collectSessionIds(surface.root, visibleIds);
      }

      const cwdStore = useTerminalCwdStore.getState();
      const osc7 = cwdStore.osc7SessionIds();
      const needed = visibleIds.filter((id) => !osc7.has(id));

      // Drop entries for sessions that no longer exist anywhere, so a long
      // session doesn't accumulate cwds for closed panes. Scoped to the
      // whole snapshot (not just the active workspace) so switching
      // workspaces doesn't discard a background pane's known directory.
      const live = new Set<string>();
      for (const ws of appState.workspaces) {
        for (const surface of ws.surfaces) {
          const ids: string[] = [];
          collectSessionIds(surface.root, ids);
          for (const id of ids) live.add(id);
        }
      }
      cwdStore.pruneCwds(live);

      if (needed.length === 0) return;

      inFlight = true;
      try {
        const result = await terminalSessionCwds(needed);
        if (!cancelled) {
          useTerminalCwdStore.getState().setPolledCwds(result);
        }
      } catch (err) {
        // A failed poll is cosmetic — keep the last known directory rather
        // than blanking the header, and let the next tick retry.
        console.debug("terminal cwd poll failed:", err);
      } finally {
        inFlight = false;
      }
    };

    // Poll once immediately so a freshly-opened pane doesn't wait a full
    // interval before its header can show anything.
    void tick();
    const timer = window.setInterval(() => void tick(), POLL_INTERVAL_MS);

    // Re-poll the moment the window comes back, instead of showing a stale
    // directory until the next scheduled tick.
    const onVisibility = () => {
      if (!document.hidden) void tick();
    };
    document.addEventListener("visibilitychange", onVisibility);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);
}
