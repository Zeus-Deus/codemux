import { useMemo, type ReactNode } from "react";
import { Check, ChevronDown, ChevronRight } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SidebarMenu } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { ProjectsModel, ProjectsRow } from "@/components/layout/sidebar-projects-model";
import { useSidebarViewStore } from "@/components/layout/sidebar-view-store";
import type { ProjectGroup } from "@/stores/app-store";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { cn } from "@/lib/utils";
import type { PluginTreeNode } from "./contract";
import {
  buildPluginTreeLayout,
  togglePluginTreeFold,
  type PluginTreeRow,
  type PluginTreeSection,
} from "./native-tree";
import { usePluginSidebarLayout } from "./plugin-sidebar-layout";
import { PluginBoundary } from "./PluginBoundary";
import { pluginProjectIdOf, workspaceChats } from "./workspace-context";

export type PluginTreeLayout = {
  readonly sections: ReadonlyArray<PluginTreeSection<ProjectGroup, WorkspaceSnapshot>>;
};

const flattenRows = (rows: ReadonlyArray<ProjectsRow>): WorkspaceSnapshot[] =>
  rows.flatMap((row) => [row.workspace, ...flattenRows(row.children)]);

/** Fold entries are stored per tree, so two trees never share a section's fold. */
const foldPrefix = (treeKey: string) => `${treeKey}\0`;

/**
 * The chosen plugin tree laid over the Projects view's live rows, or null to
 * show the built-in grouping. Rows come from the host's own model (with its
 * filter), so the tree can only arrange what the sidebar already lists.
 */
export function usePluginTreeLayout(input: {
  readonly nodes: ReadonlyArray<PluginTreeNode> | null;
  readonly treeKey: string | null;
  readonly model: ProjectsModel;
  readonly projectGroups: ReadonlyArray<ProjectGroup>;
  readonly allWorkspaces: ReadonlyArray<WorkspaceSnapshot>;
  readonly activeWorkspaceId: string;
}): PluginTreeLayout | null {
  const folds = useSidebarViewStore((state) => state.pluginTreeFolds);
  const { nodes, treeKey, model, projectGroups, allWorkspaces, activeWorkspaceId } = input;
  return useMemo(() => {
    if (nodes === null || treeKey === null) return null;
    const live = [
      ...(model.chats ? flattenRows(model.chats.rows) : []),
      ...model.projects.flatMap((section) => flattenRows(section.rows)),
      ...model.quietProjects.flatMap((section) => flattenRows(section.rows)),
    ];
    // The open workspace always shows, even when it is parked.
    const active = live.some((ws) => ws.workspace_id === activeWorkspaceId)
      ? null
      : (allWorkspaces.find((ws) => ws.workspace_id === activeWorkspaceId) ?? null);
    const items = active ? [...live, active] : live;
    const listed = new Set(items.map((ws) => ws.workspace_id));
    const rowByThread = new Map<string, string>();
    for (const ws of items) for (const chat of workspaceChats(ws)) rowByThread.set(chat.threadId, ws.workspace_id);
    const groupByProjectId = new Map<string, ProjectGroup>();
    for (const group of projectGroups) {
      for (const ws of group.workspaces) groupByProjectId.set(pluginProjectIdOf(ws), group);
    }
    const prefix = foldPrefix(treeKey);
    const scopedFolds = new Set(folds.filter((key) => key.startsWith(prefix)).map((key) => key.slice(prefix.length)));
    return buildPluginTreeLayout<ProjectGroup, WorkspaceSnapshot>({
      nodes,
      items,
      keyOf: (ws) => ws.workspace_id,
      parentKeyOf: (ws) =>
        ws.parent_workspace_id && listed.has(ws.parent_workspace_id) ? ws.parent_workspace_id : null,
      rowForThread: (threadId) => rowByThread.get(threadId) ?? null,
      liveKeys: listed,
      project: (projectId) => {
        const group = groupByProjectId.get(projectId);
        return group ? { key: group.projectPath, project: group } : null;
      },
      showsEmptyProject: () => false,
      folds: scopedFolds,
      routeKey: activeWorkspaceId || null,
    });
  }, [nodes, treeKey, model, projectGroups, allWorkspaces, activeWorkspaceId, folds]);
}

export function togglePluginTreeSection(treeKey: string, sectionKey: string, collapsedByDefault: boolean): void {
  const store = useSidebarViewStore.getState();
  const prefix = foldPrefix(treeKey);
  const mine = store.pluginTreeFolds.filter((key) => key.startsWith(prefix)).map((key) => key.slice(prefix.length));
  const others = store.pluginTreeFolds.filter((key) => !key.startsWith(prefix));
  const next = togglePluginTreeFold(mine, sectionKey, collapsedByDefault).map((key) => prefix + key);
  store.setPluginTreeFolds([...others, ...next]);
}

const toProjectsRows = (rows: ReadonlyArray<PluginTreeRow<WorkspaceSnapshot>>): ProjectsRow[] =>
  rows.map((row) => ({ workspace: row.item, children: toProjectsRows(row.children) }));

/** Indent per heading level; rows keep the Projects view's own geometry. */
const LEVEL_INDENT_PX = 12;

function SectionToggle({
  title,
  count,
  folded,
  onToggle,
  ...data
}: {
  title: string;
  count: number;
  folded: boolean;
  onToggle: () => void;
} & Record<`data-${string}`, string>) {
  return (
    <button
      type="button"
      aria-expanded={!folded}
      onClick={onToggle}
      {...data}
      className="flex h-8 w-full min-w-0 items-center gap-2 rounded-lg pl-1.5 pr-1 text-left text-body font-semibold text-foreground outline-none transition-colors duration-150 hover:bg-surface-2 focus-visible:bg-surface-2"
    >
      <ChevronRight
        aria-hidden
        className={cn(
          "size-3.5 shrink-0 text-muted-foreground/70 transition-transform duration-150 ease-drawer",
          !folded && "rotate-90",
        )}
      />
      <span className="min-w-0 flex-1 truncate">{title}</span>
      <span className="min-w-6 px-1 text-center text-label font-normal tabular-nums text-muted-foreground/70">
        {count}
      </span>
    </button>
  );
}

/**
 * The tree's sections, drawn with the Projects view's own rows and project
 * headers, so row actions, hover cards, status and activation are the host's.
 */
export function PluginTreeSections({
  layout,
  treeKey,
  title,
  renderRows,
  renderProjectHeader,
}: {
  layout: PluginTreeLayout;
  treeKey: string;
  title: string;
  renderRows: (rows: readonly ProjectsRow[]) => ReactNode;
  renderProjectHeader: (input: {
    group: ProjectGroup;
    count: number;
    folded: boolean;
    rows: readonly ProjectsRow[];
    onToggle: () => void;
  }) => ReactNode;
}) {
  return (
    <div role="region" aria-label={title} data-plugin-tree={treeKey} className="flex flex-col pb-2">
      {layout.sections.map((section) => {
        const rows = toProjectsRows(section.rows);
        const toggle = () => togglePluginTreeSection(treeKey, section.key, section.collapsedByDefault);
        const heading = section.heading;
        return (
          <div
            key={section.key}
            data-plugin-tree-section={section.key}
            style={{ paddingLeft: section.depth * LEVEL_INDENT_PX }}
          >
            {heading === null ? null : heading.kind === "project" ? (
              renderProjectHeader({
                group: heading.project,
                count: section.count,
                folded: section.folded,
                rows,
                onToggle: toggle,
              })
            ) : (
              <SectionToggle
                title={heading.kind === "group" ? heading.title : "Other chats"}
                count={section.count}
                folded={section.folded}
                onToggle={toggle}
                data-plugin-tree-heading={heading.kind}
              />
            )}
            {rows.length > 0 ? (
              <SidebarMenu
                className="pb-1"
                style={{ paddingLeft: (section.rowDepth - section.depth) * LEVEL_INDENT_PX }}
              >
                {renderRows(rows)}
              </SidebarMenu>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

/**
 * The layout picker beside the Projects toggle. Renders nothing until a
 * plugin offers a layout, so the sidebar is unchanged without one.
 */
export function PluginSidebarLayoutMenu() {
  return (
    <PluginBoundary label="Sidebar layout menu" fallback={null}>
      <SidebarLayoutMenu />
    </PluginBoundary>
  );
}

function SidebarLayoutMenu() {
  const { options, selected } = usePluginSidebarLayout();
  const remembered = useSidebarViewStore((state) => state.pluginLayout);
  const setPluginLayout = useSidebarViewStore((state) => state.setPluginLayout);
  if (options.length === 0 && remembered === null) return null;
  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Sidebar layout"
              data-plugin-layout-menu
              className="flex h-8 w-4 shrink-0 items-center justify-center rounded-md text-muted-foreground outline-none transition-colors duration-150 hover:bg-surface-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:w-8"
            >
              <ChevronDown className="size-3" />
            </button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom">Sidebar layout</TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-60">
        <DropdownMenuLabel>Projects layout</DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => setPluginLayout(null)}>
          <span className="flex-1">Projects</span>
          {remembered === null ? <Check className="size-3.5" /> : null}
        </DropdownMenuItem>
        {options.length > 0 ? <DropdownMenuSeparator /> : null}
        {options.map((option) => (
          <DropdownMenuItem key={option.key} onSelect={() => setPluginLayout(option.key)}>
            <span className="min-w-0 flex-1 truncate">
              {option.title}
              <span className="text-muted-foreground"> · {option.pluginName}</span>
            </span>
            {selected?.key === option.key ? <Check className="size-3.5" /> : null}
          </DropdownMenuItem>
        ))}
        {remembered !== null && selected === null ? (
          <DropdownMenuItem disabled>
            <span className="text-muted-foreground">Chosen layout unavailable; showing Projects</span>
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
