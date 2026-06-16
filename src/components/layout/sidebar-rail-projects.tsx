import { useMemo } from "react";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { AsciiSpinner } from "@/components/ui/ascii-spinner";
import { PrStatusIcon } from "@/components/github/pr-status-icon";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import {
  activateWorkspace,
  agentChatCreatePane,
  createEmptyWorkspace,
} from "@/tauri/commands";
import { getProjectStatus, getWorkspaceStatus } from "@/lib/pane-status";
import { useProjectAppearance } from "./use-project-appearance";
import { cn } from "@/lib/utils";
import { Cloud, GitBranch, Home, Laptop, Plus, Workflow } from "lucide-react";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/** Amber count pill for "agent finished / left a notification". */
function NotifBadge({ count }: { count: number }) {
  return (
    <span className="absolute -top-1 -right-1 flex h-[15px] min-w-[15px] items-center justify-center rounded-full bg-warning px-1 text-[9px] font-semibold leading-none text-background tabular-nums">
      {count > 9 ? "9+" : count}
    </span>
  );
}

/** Leading glyph for a workspace inside the flyout, mirroring the
 *  expanded sidebar row's icon rules. */
function flyoutWorkspaceIcon(
  workspace: WorkspaceSnapshot,
  status: ActivePaneStatus | null,
) {
  if (status === "working") return <AsciiSpinner />;
  const isPrimary = !workspace.worktree_path;
  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  if (isRemote)
    return <Cloud className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
  if (workspace.workspace_type === "open_flow")
    return <Workflow className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
  if (isPrimary)
    return <Laptop className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
  if (workspace.pr_state)
    return <PrStatusIcon state={workspace.pr_state} size={3.5} />;
  return <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />;
}

/** One workspace row inside the project flyout. Subscribes to its own
 *  agent status so a working/needs-input/done state stays live while the
 *  flyout is open — answering "can I still see agents working when the
 *  sidebar is collapsed?". */
function RailFlyoutWorkspaceRow({
  workspace,
  isActive,
}: {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
}) {
  const status = useAppStore((s) =>
    s.appState
      ? getWorkspaceStatus(workspace.surfaces, s.appState.pane_statuses)
      : null,
  );

  const handleClick = () => {
    useChatDraftStore.getState().setActiveDraft(null);
    activateWorkspace(workspace.workspace_id).catch(console.error);
  };

  return (
    <button
      type="button"
      onClick={handleClick}
      className={cn(
        "flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-muted/40",
        isActive && "bg-muted",
      )}
    >
      <span className="relative flex size-4 shrink-0 items-center justify-center">
        {flyoutWorkspaceIcon(workspace, status)}
        {status && status !== "working" && (
          <StatusIndicator
            status={status}
            withTooltip={false}
            className="absolute -top-1 -right-1"
          />
        )}
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span
            className={cn(
              "truncate text-[12px] leading-tight",
              isActive ? "font-medium text-foreground" : "text-foreground/85",
            )}
          >
            {workspace.title}
          </span>
          {workspace.notification_count > 0 && (
            <span className="ml-auto shrink-0 rounded-full bg-warning/15 px-1.5 text-[10px] leading-[14px] text-warning tabular-nums">
              {workspace.notification_count}
            </span>
          )}
        </span>
        {workspace.git_branch && (
          <span className="block truncate font-mono text-[10px] leading-tight text-muted-foreground/60">
            {workspace.git_branch}
          </span>
        )}
      </span>
    </button>
  );
}

/** The popover that opens when hovering a collapsed project avatar —
 *  lists the project's workspaces with live status and a "new workspace"
 *  action, so the rail stays fully operable without expanding. */
function RailProjectFlyout({
  projectName,
  projectPath,
  workspaces,
  activeWorkspaceId,
}: {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
}) {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );

  const handleNew = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (e.shiftKey || !enableAgentChat) {
      setShowNewWorkspaceDialog(true, projectPath);
      return;
    }
    if (enableLazyWorkspaceCreation) {
      const store = useChatDraftStore.getState();
      const draft = store.getOrCreateProjectDraft(projectPath);
      store.setActiveDraft(draft.draftId);
      return;
    }
    try {
      const wsId = await createEmptyWorkspace(projectPath);
      await activateWorkspace(wsId);
      await agentChatCreatePane(wsId, null, projectPath);
    } catch (err) {
      console.error("[rail] failed to open chat pane:", err);
      setShowNewWorkspaceDialog(true, projectPath);
    }
  };

  return (
    <div className="flex flex-col">
      <div className="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
        <span className="truncate text-[13px] font-medium text-foreground">
          {projectName}
        </span>
        <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground/60">
          {workspaces.length}
        </span>
      </div>
      <div className="max-h-[320px] overflow-y-auto py-1">
        {workspaces.map((ws) => (
          <RailFlyoutWorkspaceRow
            key={ws.workspace_id}
            workspace={ws}
            isActive={ws.workspace_id === activeWorkspaceId}
          />
        ))}
      </div>
      <button
        type="button"
        onClick={handleNew}
        className="flex items-center gap-2 border-t border-border px-3 py-2 text-[12px] text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
      >
        <Plus className="h-3.5 w-3.5" />
        New workspace
      </button>
    </div>
  );
}

/** A single project as an avatar button in the collapsed rail. Shows an
 *  aggregate status dot (or notification count) so agent activity stays
 *  visible while collapsed; click jumps to the project's active/first
 *  workspace; hover opens the flyout. */
function RailProjectItem({
  projectName,
  projectPath,
  workspaces,
  activeWorkspaceId,
}: {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
}) {
  const homeDir = useHomeDir();
  const isHomeGroup = projectName === "Home" && projectPath === homeDir;
  const { customColor, imageUrl, imageVersion } =
    useProjectAppearance(projectPath);

  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses);
  const aggStatus = useMemo(
    () => (paneStatuses ? getProjectStatus(workspaces, paneStatuses) : null),
    [workspaces, paneStatuses],
  );
  const aggNotif = useMemo(
    () =>
      workspaces.reduce((sum, w) => sum + (w.notification_count ?? 0), 0),
    [workspaces],
  );
  const isActiveProject = workspaces.some(
    (w) => w.workspace_id === activeWorkspaceId,
  );

  const handleClick = () => {
    const jump =
      workspaces.find((w) => w.workspace_id === activeWorkspaceId) ??
      workspaces[0];
    if (!jump) return;
    useChatDraftStore.getState().setActiveDraft(null);
    activateWorkspace(jump.workspace_id).catch(console.error);
  };

  // Single corner indicator, by urgency:
  // needs-input (red) > notification count (amber) > working (amber pulse)
  // > ready-for-review (green). The flyout carries the per-workspace detail.
  let corner: React.ReactNode = null;
  if (aggStatus === "permission") {
    corner = (
      <StatusIndicator
        status="permission"
        withTooltip={false}
        className="absolute -top-1 -right-1"
      />
    );
  } else if (aggNotif > 0) {
    corner = <NotifBadge count={aggNotif} />;
  } else if (aggStatus === "working") {
    corner = (
      <StatusIndicator
        status="working"
        withTooltip={false}
        className="absolute -top-1 -right-1"
      />
    );
  } else if (aggStatus === "review") {
    corner = (
      <StatusIndicator
        status="review"
        withTooltip={false}
        className="absolute -top-1 -right-1"
      />
    );
  }

  return (
    <HoverCard openDelay={150} closeDelay={120}>
      <HoverCardTrigger asChild>
        <button
          type="button"
          onClick={handleClick}
          aria-label={`${projectName} — ${workspaces.length} workspace${
            workspaces.length === 1 ? "" : "s"
          }`}
          className={cn(
            "relative flex size-8 items-center justify-center rounded-lg transition-colors hover:bg-sidebar-accent",
            isActiveProject && "bg-muted",
          )}
        >
          {isHomeGroup ? (
            <div className="flex size-6 items-center justify-center rounded-full border border-border bg-muted">
              <Home className="h-3.5 w-3.5 text-muted-foreground" />
            </div>
          ) : (
            <ProjectAvatar
              name={projectName}
              color={customColor}
              imageUrl={imageUrl}
              cacheBust={imageVersion}
              size="lg"
              shape="circle"
            />
          )}
          {corner}
        </button>
      </HoverCardTrigger>
      <HoverCardContent side="right" align="start" sideOffset={8} className="w-64 p-0">
        <RailProjectFlyout
          projectName={projectName}
          projectPath={projectPath}
          workspaces={workspaces}
          activeWorkspaceId={activeWorkspaceId}
        />
      </HoverCardContent>
    </HoverCard>
  );
}

/**
 * Collapsed (icon-rail) rendering of the project list. Each project becomes
 * an avatar with an aggregate agent-status dot / notification badge; hovering
 * opens a flyout to see and switch between its workspaces without expanding.
 */
export function SidebarRailProjects() {
  const appState = useAppStore((s) => s.appState);
  const allWorkspaces = appState?.workspaces ?? [];
  const homeDir = useHomeDir();
  const projectGroups = useProjectGroupedWorkspaces(allWorkspaces, homeDir);
  const activeWorkspaceId = appState?.active_workspace_id ?? "";

  return (
    <div className="flex flex-col items-center gap-1 px-1 py-2">
      {projectGroups.map((group) => (
        <RailProjectItem
          key={group.projectPath}
          projectName={group.projectName}
          projectPath={group.projectPath}
          workspaces={group.workspaces}
          activeWorkspaceId={activeWorkspaceId}
        />
      ))}
    </div>
  );
}
