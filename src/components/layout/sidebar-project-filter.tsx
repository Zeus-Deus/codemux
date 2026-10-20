import { useState } from "react";
import { Check, ChevronDown, Folder, FolderOpen, FolderPlus } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import type { ProjectGroup } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useProjectAppearance } from "./use-project-appearance";

/** A project's mini square avatar, resolving its customised appearance. Kept
 *  as a per-item component so the appearance hook always runs unconditionally
 *  (each avatar owns its own hook rather than looping hooks in the parent). */
export function ProjectMiniAvatar({ name, path }: { name: string; path: string }) {
  const appearance = useProjectAppearance(path);
  return (
    <ProjectAvatar
      name={name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      size="sm"
      shape="square"
      className="font-bold"
    />
  );
}

interface ProjectFilterItemProps {
  name: string;
  path: string;
  /** Number of visible cards in this project, including pinned parked work. */
  count: number;
  active: boolean;
  onSelect: () => void;
}

/** One project row in the filter dropdown: mini avatar, name, and its active
 *  workspace count. */
function ProjectFilterItem({
  name,
  path,
  count,
  active,
  onSelect,
}: ProjectFilterItemProps) {
  return (
    <DropdownMenuItem
      onClick={onSelect}
      aria-label={name}
      // A single-choice filter: the check marks the selected row, and the
      // radio role is what tells assistive tech which one that is.
      role="menuitemradio"
      aria-checked={active}
      className={cn(
        "h-8 gap-2 rounded-md px-2 text-label",
        active && "font-medium text-foreground",
      )}
    >
      <ProjectMiniAvatar name={name} path={path} />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      <span className="shrink-0 text-caption tabular-nums text-muted-foreground">
        {count}
      </span>
      <span className="size-4 shrink-0" aria-hidden>
        {active && <Check className="size-4" />}
      </span>
    </DropdownMenuItem>
  );
}

/** Active (unparked) workspace count per project plus the grand total, as the
 *  filter menu shows them. Parking is the only thing that decrements a count;
 *  a pinned workspace always counts. */
export function computeProjectCounts(
  projectGroups: ProjectGroup[],
  settledIds: ReadonlySet<string>,
  snoozedIds: ReadonlySet<string>,
): { map: Map<string, number>; total: number } {
  const map = new Map<string, number>();
  let total = 0;
  for (const group of projectGroups) {
    let count = 0;
    for (const ws of group.workspaces) {
      if (
        ws.pinned_at != null ||
        (!settledIds.has(ws.workspace_id) && !snoozedIds.has(ws.workspace_id))
      ) {
        count += 1;
      }
    }
    map.set(group.projectPath, count);
    total += count;
  }
  return { map, total };
}

interface SidebarProjectFilterProps {
  projectGroups: ProjectGroup[];
  counts: { map: Map<string, number>; total: number };
  /** Active filter (project path) or null for "All projects". */
  filter: string | null;
  filterName: string | null;
  setFilter: (projectPath: string | null) => void;
}

/** The "All projects" dropdown at the start of the sidebar header row. Shared
 *  by the Activity and Projects views so both scope by the same filter. */
export function SidebarProjectFilter({
  projectGroups,
  counts,
  filter,
  filterName,
  setFilter,
}: SidebarProjectFilterProps) {
  const setShowNewProjectScreen = useUIStore((s) => s.setShowNewProjectScreen);
  const { openProject } = useProjectActions();
  // Whether the dropdown is open — drives the trigger chevron rotation.
  const [filterMenuOpen, setFilterMenuOpen] = useState(false);

  return (
    <DropdownMenu open={filterMenuOpen} onOpenChange={setFilterMenuOpen}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label="Filter by project"
          data-project-filter
          className={cn(
            "flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2.5",
            "border border-transparent bg-transparent text-label font-semibold text-foreground/80",
            "transition-colors duration-150 hover:border-border/60 hover:bg-surface-2",
          )}
        >
          {filter === null ? (
            <Folder className="size-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <ProjectMiniAvatar name={filterName ?? ""} path={filter} />
          )}
          <span className="min-w-0 flex-1 truncate text-left">
            {filter === null ? "All projects" : filterName}
          </span>
          <ChevronDown
            className={cn(
              "size-3 shrink-0 text-muted-foreground transition-transform duration-150",
              filterMenuOpen && "rotate-180",
            )}
          />
        </button>
      </DropdownMenuTrigger>
      {/* Capped at a menu-sized panel rather than growing with the
          project count: past a dozen or so projects an uncapped menu
          stretches to the window edge and reads as clipped. The cap is
          whichever is smaller — a comfortable ~9 rows, or whatever room
          Radix actually has — so the same panel behaves in a short
          window. */}
      <DropdownMenuContent
        side="bottom"
        align="start"
        className="flex max-h-[min(21rem,var(--radix-dropdown-menu-content-available-height))] w-[248px] flex-col overflow-y-hidden p-0"
      >
        {/* "All projects" is the reset, so it stays pinned above the
            scroll area — scrolling to the top to clear a filter is the
            kind of small tax that makes a long list feel like work. */}
        <div className="shrink-0 border-b border-border/60 p-1.5">
          <DropdownMenuItem
            onClick={() => setFilter(null)}
            aria-label="All projects"
            role="menuitemradio"
            aria-checked={filter === null}
            className={cn(
              "h-8 gap-2 rounded-md px-2 text-label",
              filter === null && "font-medium text-foreground",
            )}
          >
            <Folder className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate">All projects</span>
            <span className="shrink-0 text-caption tabular-nums text-muted-foreground">
              {counts.total}
            </span>
            <span className="size-4 shrink-0" aria-hidden>
              {filter === null && <Check className="size-4" />}
            </span>
          </DropdownMenuItem>
        </div>
        <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto p-1.5">
          {projectGroups.map((group) => (
            <ProjectFilterItem
              key={group.projectPath}
              name={group.projectName}
              path={group.projectPath}
              count={counts.map.get(group.projectPath) ?? 0}
              active={filter === group.projectPath}
              onSelect={() => setFilter(group.projectPath)}
            />
          ))}
        </div>
        {/* Project creation is pinned under the list, mirroring the
            pinned reset above it. Single-line rows sized like the
            project rows; the labels themselves carry the difference
            (adopt what's on disk vs start or clone a new repo). */}
        <div className="shrink-0 border-t border-border/60 p-1.5">
          <DropdownMenuItem
            onClick={() => openProject()}
            className="h-8 gap-2 rounded-md px-2 text-label"
          >
            <FolderOpen className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate">Open existing project</span>
            <MenuKeycap actionId="openProject" />
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() => setShowNewProjectScreen(true)}
            className="h-8 gap-2 rounded-md px-2 text-label"
          >
            <FolderPlus className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate">Create or clone project</span>
          </DropdownMenuItem>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
