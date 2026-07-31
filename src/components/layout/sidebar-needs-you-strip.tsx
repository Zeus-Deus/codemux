import { startTransition, useEffect, useMemo, useState } from "react";
import { ArrowDown } from "lucide-react";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { getWorkspaceStatus } from "@/lib/pane-status";
import {
  useSidebarDensityStore,
  formatElapsed,
  permissionBlockerText,
} from "@/stores/sidebar-density-store";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { useAppStore } from "@/stores/app-store";
import type { ProjectGroup } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { activateWorkspace, dbGetUiState } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface StripEntry {
  workspace: WorkspaceSnapshot;
  projectName: string;
  projectPath: string;
}

interface Props {
  /** The same project grouping the list renders. The strip re-uses it to
   *  keep a stable tree order and to source each entry's project chip
   *  without recomputing the grouping. */
  projectGroups: ProjectGroup[];
  /** The inbox's active repo filter (project path), or null for "All
   *  projects". The strip scopes to it so a filtered sidebar never points at
   *  a workspace the list below is hiding. */
  filterPath?: string | null;
}

/** How many animation frames the jump waits for the target row to mount
 *  (after its group expands) before giving up — keeps the retry loop bounded
 *  so it never spins if the row never appears. ~0.5s at 60fps. */
const JUMP_SCROLL_MAX_FRAMES = 30;

/** How many jump-links the strip renders. It is pinned above the scrolling
 *  list, so an uncapped strip could swallow the sidebar on a bad day (every
 *  agent blocked at once). The header count always reports the true total, so
 *  the cap hides rows but never hides the number. Oldest-blocked wins the
 *  visible slots — the agent that has been stuck longest is the one to
 *  unblock first. */
const MAX_VISIBLE_ENTRIES = 4;

/** Jump to the blocked workspace: activate it (clearing any active chat
 *  draft, exactly like a card click) and smooth-scroll its card into view.
 *  The card never moves — the strip only points at it, which is what lets the
 *  list keep a stable, status-blind order. */
function jumpToWorkspace(workspaceId: string, projectPath: string) {
  useChatDraftStore.getState().setActiveDraft(null);
  // Ask the target's project to expand. A no-op in the flat inbox (it has no
  // collapsible groups), but harmless and still correct for any grouped list:
  // a row inside a collapsed group isn't rendered, so the scroll below would
  // silently do nothing.
  useUIStore.getState().requestExpandProject(projectPath);
  startTransition(() => {
    activateWorkspace(workspaceId).catch(console.error);
  });
  // The row may not exist yet — the group was just asked to expand and React
  // needs a commit to mount it. Retry across a few frames until the row is in
  // the DOM, then scroll; give up after a small cap so we never spin.
  // `scrollIntoView` is a no-op in jsdom, so guard it.
  let frames = 0;
  const tryScroll = () => {
    // Two selectors because the strip outlives one list rendering: the flat
    // inbox marks its cards `data-inbox-card`, the legacy project tree marked
    // its rows `data-ws-id`. Whichever list is mounted, the jump lands.
    const escaped = CSS.escape(workspaceId);
    const el = document.querySelector<HTMLElement>(
      `[data-inbox-card="${escaped}"], [data-ws-id="${escaped}"]`,
    );
    if (el) {
      try {
        el.scrollIntoView({ block: "nearest", behavior: "smooth" });
      } catch {
        /* jsdom / unsupported — the activation already happened */
      }
      return;
    }
    if (frames++ < JUMP_SCROLL_MAX_FRAMES) requestAnimationFrame(tryScroll);
  };
  requestAnimationFrame(tryScroll);
}

/**
 * Pinned "Needs you" strip — rendered inside the sidebar's sticky header
 * block, above the workspace list, only while ≥1 workspace is waiting on the
 * user (workspace status `permission`). Absent otherwise.
 *
 * Each entry is a jump-link (project chip + blocker summary + age) that
 * activates and scrolls to the blocked workspace. The workspace's own card
 * stays exactly where it is, so the strip surfaces blocked work WITHOUT
 * reordering the list — the whole point. Because it is pinned, blocked work
 * stays one click away however far down the list you have scrolled.
 */
export function SidebarNeedsYouStrip({
  projectGroups,
  filterPath = null,
}: Props) {
  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses);
  const statusSince = useSidebarDensityStore((s) => s.statusSince);
  const observeStatus = useSidebarDensityStore((s) => s.observeStatus);

  // Collect every workspace whose aggregate status is `permission` (walking
  // the same project groups the list renders), then sort the entries
  // oldest-blocked-first by each blocker's permission timestamp from the
  // density store. Tree order is NOT blocked order — the active list runs
  // newest-first and a block can land on any card at any time — so the sort
  // is what actually puts the agent that has been stuck waiting longest at
  // the top, and (with the visibility cap below) is what guarantees the
  // longest-waiting blocker can never be hidden behind "+N more below".
  //
  // A blocker whose timestamp is not seeded yet ranks as newest until the
  // seeding effect below stamps it (it just started blocking as far as we can
  // observe); ties keep stable tree order. Note this is the ONE surface that
  // orders by status at all; the list below stays deliberately status-blind,
  // and the strip exists precisely so it can.
  const entries = useMemo<StripEntry[]>(() => {
    if (!paneStatuses) return [];
    const out: StripEntry[] = [];
    for (const group of projectGroups) {
      if (filterPath !== null && group.projectPath !== filterPath) continue;
      for (const workspace of group.workspaces) {
        if (getWorkspaceStatus(workspace.surfaces, paneStatuses) === "permission") {
          out.push({
            workspace,
            projectName: group.projectName,
            projectPath: group.projectPath,
          });
        }
      }
    }
    return out
      .map((entry, treeIndex) => {
        const mark = statusSince[entry.workspace.workspace_id];
        return {
          entry,
          treeIndex,
          blockedAt:
            mark?.status === "permission"
              ? mark.at
              : Number.POSITIVE_INFINITY,
        };
      })
      .sort(
        // `blockedAt - blockedAt` is NaN when both are unseeded (Infinity);
        // NaN and 0 both fall through `||` to the stable tree-order tie-break.
        (a, b) => a.blockedAt - b.blockedAt || a.treeIndex - b.treeIndex,
      )
      .map(({ entry }) => entry);
  }, [projectGroups, paneStatuses, filterPath, statusSince]);

  // Per-project colors, sourced from the same UI-state key the sidebar group
  // header uses (`project.color:<path>`). Fetched lazily for the projects
  // that actually have a blocked workspace; `ProjectAvatar` falls back to a
  // neutral tile when a project has no custom color.
  const projectPaths = useMemo(
    () => [...new Set(entries.map((e) => e.projectPath))],
    [entries],
  );
  const projectPathsKey = projectPaths.join("\0");
  const [colors, setColors] = useState<Record<string, string | null>>({});
  useEffect(() => {
    let cancelled = false;
    Promise.all(
      projectPaths.map(async (path) => {
        try {
          const color = await dbGetUiState(`project.color:${path}`);
          return [path, color || null] as const;
        } catch {
          return [path, null] as const;
        }
      }),
    ).then((pairs) => {
      if (!cancelled) setColors(Object.fromEntries(pairs));
    });
    return () => {
      cancelled = true;
    };
    // projectPathsKey is a stable digest of projectPaths — re-fetch only when
    // the set of blocked projects changes, not on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectPathsKey]);

  // Seed each listed blocker's status timestamp so the age advances even when
  // its row never mounts. The per-row `observeStatus` effect only runs for
  // rendered rows, so a blocker inside a *collapsed* project group would
  // otherwise read "0s" forever (and flicker 0s for one frame on first show).
  // `observeStatus` is a no-op when the status is unchanged, so seeding here
  // can't fight a mounted row's own observation or reset the timestamp.
  const blockedIdsKey = entries.map((e) => e.workspace.workspace_id).join(" ");
  useEffect(() => {
    for (const { workspace } of entries) {
      observeStatus(workspace.workspace_id, "permission");
    }
    // Keyed on the set of blocked ids, not the entries array identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [blockedIdsKey, observeStatus]);

  // Coarse (~30s) clock so the age labels stay current without a per-entry
  // timer; only ticks while the strip is visible.
  const now = useCoarseClock(entries.length > 0);

  // Absent / zero-height when nothing needs the user.
  if (entries.length === 0) return null;

  return (
    <div
      className="mx-1.5 mt-1.5 mb-1.5 rounded-[10px] border border-status-attention/25 bg-status-attention/5 px-2 py-[7px] animate-in fade-in slide-in-from-top-1 duration-200"
      role="region"
      aria-label="Workspaces needing you"
    >
      {/* Header line: small red dot + mono uppercase label with a count. */}
      <div className="flex items-center gap-1.5 px-1 pb-1">
        <span className="size-1.5 rounded-full bg-status-attention shrink-0" />
        <span className="font-mono text-[10px] font-semibold tracking-[0.14em] text-status-attention">
          NEEDS YOU · {entries.length}
        </span>
      </div>

      <div className="flex flex-col">
        {entries
          .slice(0, MAX_VISIBLE_ENTRIES)
          .map(({ workspace, projectName, projectPath }) => {
            const mark = statusSince[workspace.workspace_id];
            const age = formatElapsed(mark != null ? now - mark.at : 0);
            return (
              <button
                key={workspace.workspace_id}
                type="button"
                onClick={() => jumpToWorkspace(workspace.workspace_id, projectPath)}
                className="group/needs flex items-center gap-2 rounded px-1 py-[3px] text-left hover:bg-foreground/5 transition-colors"
                aria-label={`Jump to ${workspace.title} — waiting for your input`}
              >
                <ProjectAvatar
                  name={projectName}
                  color={colors[projectPath] ?? null}
                  size="sm"
                  shape="square"
                  className="font-bold"
                />
                <span className="min-w-0 flex-1 truncate text-[12px] text-foreground">
                  {permissionBlockerText(workspace)}
                </span>
                <span className="ml-auto flex items-center gap-1 shrink-0 font-mono text-[10px] text-muted-foreground tabular-nums">
                  {age}
                  <ArrowDown className="size-2.5" aria-hidden />
                </span>
              </button>
            );
          })}
        {/* Never let the cap hide work silently — say how many blocked
            workspaces are only reachable by scrolling. */}
        {entries.length > MAX_VISIBLE_ENTRIES && (
          <span className="px-1 pt-1 font-mono text-[10px] text-status-attention/70">
            +{entries.length - MAX_VISIBLE_ENTRIES} more below
          </span>
        )}
      </div>
    </div>
  );
}
