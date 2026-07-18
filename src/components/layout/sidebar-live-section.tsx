import { useEffect, useMemo, useState } from "react";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { dbGetUiState } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

export interface LiveEntry {
  workspace: WorkspaceSnapshot;
  projectName: string;
  projectPath: string;
}

interface Props {
  /** Live rows, already sorted permission → working → review and stable by
   *  tree order within each bucket (computed by the list). */
  entries: LiveEntry[];
  activeWorkspaceId: string;
}

/**
 * The `LIVE` section rendered above the project tree in "gather on top" mode.
 * Each entry is the SAME `SidebarWorkspaceRow` (full affordances) plus a
 * leading project chip so its origin stays visible. These rows are mirrors of
 * tree membership: they carry no `data-ws-id` / `data-ws-index` and no
 * draggable wrapper, so the drag-and-drop DOM (scoped to the project zones
 * below) never double-matches them.
 */
export function SidebarLiveSection({ entries, activeWorkspaceId }: Props) {
  // Per-project colors for the chips, from the same UI-state key the group
  // header + needs-you strip use. Fetched lazily for the projects on screen.
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
    // the set of live projects changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectPathsKey]);

  if (entries.length === 0) return null;

  return (
    <div
      className="pb-1"
      role="region"
      aria-label="Live agents"
    >
      <div className="flex items-center gap-1.5 px-2 pb-1 pt-1">
        <span className="font-mono text-[9.5px] font-semibold uppercase tracking-[0.14em] text-accent-ember">
          LIVE
        </span>
        <span className="font-mono text-[9.5px] font-semibold tracking-[0.14em] text-muted-foreground">
          · {entries.length}
        </span>
      </div>
      {entries.map(({ workspace, projectName, projectPath }) => (
        <SidebarWorkspaceRow
          key={workspace.workspace_id}
          workspace={workspace}
          isActive={workspace.workspace_id === activeWorkspaceId}
          projectChip={{
            name: projectName,
            color: colors[projectPath] ?? null,
          }}
        />
      ))}
    </div>
  );
}
