import { useEffect, useRef, useState } from "react";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useAppStore } from "@/stores/app-store";

/**
 * One polite live region for the state that matters most: an agent blocked
 * on the user. The sidebar shows "Needs you" in red, but that text is static
 * and a screen-reader user working elsewhere would never hear it.
 *
 * Announces once per transition into `permission`, per workspace. Workspaces
 * already blocked when the app loads are not announced, and a workspace that
 * stays blocked across snapshots is not announced again.
 */
export function NeedsYouAnnouncer() {
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses);
  const blockedRef = useRef<Set<string> | null>(null);
  const [message, setMessage] = useState({ text: "", seq: 0 });

  useEffect(() => {
    if (!workspaces || !paneStatuses) return;
    const blocked = new Set<string>();
    const newlyBlocked: string[] = [];
    for (const ws of workspaces) {
      if (getWorkspaceStatus(ws.surfaces, paneStatuses) !== "permission") continue;
      blocked.add(ws.workspace_id);
      if (blockedRef.current && !blockedRef.current.has(ws.workspace_id)) {
        newlyBlocked.push(`${ws.title} needs you`);
      }
    }
    blockedRef.current = blocked;
    if (newlyBlocked.length > 0) {
      setMessage((prev) => ({ text: newlyBlocked.join(". "), seq: prev.seq + 1 }));
    }
  }, [workspaces, paneStatuses]);

  return (
    <div role="status" aria-live="polite" className="sr-only">
      {/* Keyed so a repeat of the same sentence is still a new node to
          announce, not an unchanged text node the reader skips. */}
      {message.text && <span key={message.seq}>{message.text}</span>}
    </div>
  );
}
