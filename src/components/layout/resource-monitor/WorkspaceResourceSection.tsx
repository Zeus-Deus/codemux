import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import type {
  ResourceSessionMetrics,
  ResourceWorkspaceMetrics,
} from "@/tauri/types";
import type { SortOption } from "./types";
import { formatCpu, formatMemory } from "./utils/formatters";
import { getUsageSeverity } from "./utils/resource-severity";
import { UsageSeverityBadge } from "./UsageSeverityBadge";

const METRIC_COLS = "flex items-center shrink-0 tabular-nums tracking-tight";
const CPU_COL = "w-12 text-right";
const MEM_COL = "w-16 text-right";

interface ProjectResourceGroup {
  projectId: string;
  projectName: string;
  cpu: number;
  memory: number;
  workspaces: ResourceWorkspaceMetrics[];
}

interface WorkspaceResourceSectionProps {
  workspaces: ResourceWorkspaceMetrics[];
  sortOption: SortOption;
  collapsedProjects: Set<string>;
  toggleProject: (projectId: string) => void;
  collapsedWorkspaces: Set<string>;
  toggleWorkspace: (workspaceId: string) => void;
  navigateToWorkspace: (workspaceId: string) => void;
  navigateToSession: (sessionId: string) => void;
  getSessionName: (session: ResourceSessionMetrics) => string;
}

function groupWorkspacesByProject(
  workspaces: ResourceWorkspaceMetrics[],
): ProjectResourceGroup[] {
  const projectMap = new Map<string, ProjectResourceGroup>();

  for (const workspace of workspaces) {
    const projectId = workspace.project_id || "unknown";
    const projectName = workspace.project_name || "Project";
    let group = projectMap.get(projectId);
    if (!group) {
      group = { projectId, projectName, cpu: 0, memory: 0, workspaces: [] };
      projectMap.set(projectId, group);
    }
    group.cpu += workspace.cpu;
    group.memory += workspace.memory;
    group.workspaces.push(workspace);
  }

  return [...projectMap.values()];
}

function sortWorkspaces(
  workspaces: ResourceWorkspaceMetrics[],
  sortOption: SortOption,
): ResourceWorkspaceMetrics[] {
  const sorted = [...workspaces];
  switch (sortOption) {
    case "memory":
      sorted.sort((a, b) => b.memory - a.memory);
      break;
    case "cpu":
      sorted.sort((a, b) => b.cpu - a.cpu);
      break;
    case "name":
      sorted.sort((a, b) => a.workspace_name.localeCompare(b.workspace_name));
      break;
  }
  return sorted;
}

function sortProjectGroups(
  groups: ProjectResourceGroup[],
  sortOption: SortOption,
): ProjectResourceGroup[] {
  const sorted = [...groups];
  switch (sortOption) {
    case "memory":
      sorted.sort((a, b) => b.memory - a.memory);
      break;
    case "cpu":
      sorted.sort((a, b) => b.cpu - a.cpu);
      break;
    case "name":
      sorted.sort((a, b) => a.projectName.localeCompare(b.projectName));
      break;
  }
  return sorted;
}

function getProjectTotals(projects: ProjectResourceGroup[]) {
  return projects.reduce(
    (acc, project) => ({
      cpu: acc.cpu + project.cpu,
      memory: acc.memory + project.memory,
    }),
    { cpu: 0, memory: 0 },
  );
}

/** Project → Workspace → Session collapsible tree of terminal process usage. */
export function WorkspaceResourceSection({
  workspaces,
  sortOption,
  collapsedProjects,
  toggleProject,
  collapsedWorkspaces,
  toggleWorkspace,
  navigateToWorkspace,
  navigateToSession,
  getSessionName,
}: WorkspaceResourceSectionProps) {
  const projectGroups = sortProjectGroups(
    groupWorkspacesByProject(workspaces),
    sortOption,
  ).map((group) => ({
    ...group,
    workspaces: sortWorkspaces(group.workspaces, sortOption),
  }));
  const projectTotals = getProjectTotals(projectGroups);

  return projectGroups.map((project, projectIndex) => {
    const isProjectCollapsed = collapsedProjects.has(project.projectId);
    const projectSeverity = getUsageSeverity(project, projectTotals);

    return (
      <div
        key={project.projectId}
        className={cn("py-1", projectIndex > 0 && "border-t border-border/40")}
      >
        <button
          type="button"
          onClick={() => toggleProject(project.projectId)}
          className="group w-full flex items-center justify-between px-2 py-1.5 hover:bg-foreground/[0.04] transition-colors"
          aria-label={isProjectCollapsed ? "Expand project" : "Collapse project"}
        >
          <div className="flex items-center gap-1 min-w-0 mr-2">
            <span className="flex items-center justify-center h-4 w-4 shrink-0 text-muted-foreground/70 group-hover:text-muted-foreground transition-colors">
              {isProjectCollapsed ? (
                <ChevronRight className="h-3 w-3" />
              ) : (
                <ChevronDown className="h-3 w-3" />
              )}
            </span>
            <span className="text-[11px] font-semibold uppercase tracking-[0.04em] truncate min-w-0 text-muted-foreground">
              {project.projectName}
            </span>
            <UsageSeverityBadge severity={projectSeverity} />
          </div>
          <div className={cn(METRIC_COLS, "text-[12px] text-foreground/90")}>
            <span className={CPU_COL}>{formatCpu(project.cpu)}</span>
            <span className={MEM_COL}>{formatMemory(project.memory)}</span>
          </div>
        </button>

        {!isProjectCollapsed &&
          project.workspaces.map((workspace) => {
            const isCollapsed = collapsedWorkspaces.has(workspace.workspace_id);
            const workspaceSeverity = getUsageSeverity(workspace, project);
            const hasSessions = workspace.sessions.length > 0;

            return (
              <div key={workspace.workspace_id}>
                <div className="group flex items-center hover:bg-foreground/[0.04] transition-colors">
                  {hasSessions ? (
                    <button
                      type="button"
                      onClick={() => toggleWorkspace(workspace.workspace_id)}
                      className="flex items-center justify-center h-7 w-5 ml-3.5 shrink-0 text-muted-foreground/60 hover:text-muted-foreground transition-colors"
                      aria-label={
                        isCollapsed ? "Expand workspace" : "Collapse workspace"
                      }
                    >
                      {isCollapsed ? (
                        <ChevronRight className="h-3 w-3" />
                      ) : (
                        <ChevronDown className="h-3 w-3" />
                      )}
                    </button>
                  ) : (
                    <span className="h-7 w-5 ml-3.5 shrink-0" />
                  )}
                  <button
                    type="button"
                    onClick={() => navigateToWorkspace(workspace.workspace_id)}
                    className="flex-1 min-w-0 flex items-center justify-between py-1.5 pr-3.5 pl-1 text-left"
                  >
                    <div className="flex items-center gap-1.5 min-w-0 mr-2">
                      <span className="text-[12px] text-foreground truncate min-w-0">
                        {workspace.workspace_name}
                      </span>
                      <UsageSeverityBadge severity={workspaceSeverity} />
                    </div>
                    <div
                      className={cn(METRIC_COLS, "text-[12px] text-foreground/85")}
                    >
                      <span className={CPU_COL}>{formatCpu(workspace.cpu)}</span>
                      <span className={MEM_COL}>
                        {formatMemory(workspace.memory)}
                      </span>
                    </div>
                  </button>
                </div>

                {!isCollapsed &&
                  workspace.sessions.map((session) => {
                    const sessionSeverity = getUsageSeverity(session, workspace);

                    return (
                      <button
                        type="button"
                        key={session.session_id}
                        onClick={() => navigateToSession(session.session_id)}
                        className="w-full flex items-center justify-between pl-12 pr-3.5 py-1 hover:bg-foreground/[0.04] transition-colors text-left"
                      >
                        <div className="flex items-center gap-1.5 min-w-0 mr-2">
                          <span className="h-1 w-1 shrink-0 rounded-full bg-muted-foreground/40" />
                          <span className="text-[11px] text-muted-foreground truncate min-w-0">
                            {getSessionName(session)}
                          </span>
                          <UsageSeverityBadge severity={sessionSeverity} />
                        </div>
                        <div
                          className={cn(
                            METRIC_COLS,
                            "text-[11px] text-muted-foreground/80",
                          )}
                        >
                          <span className={CPU_COL}>
                            {formatCpu(session.cpu)}
                          </span>
                          <span className={MEM_COL}>
                            {formatMemory(session.memory)}
                          </span>
                        </div>
                      </button>
                    );
                  })}
              </div>
            );
          })}
      </div>
    );
  });
}
