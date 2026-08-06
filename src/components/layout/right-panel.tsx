import { memo } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Check, Loader2, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useUIStore, type RightPanelTab } from "@/stores/ui-store";
import { ChangesPanel } from "@/components/workspace/changes-panel";
import { FileTreePanel } from "@/components/workspace/file-tree-panel";
import { ReviewPanel } from "@/components/workspace/review-panel";
import { OrchestrationPanel } from "@/components/workflow/orchestration-panel";
import { TasksPanel } from "@/components/chat/TasksPanel";
import { useWorkspaceWorkflow } from "@/components/workflow/use-workspace-workflow";
import { useActiveChatTasks } from "@/hooks/use-active-chat-tasks";
import type {
  WorkspaceSnapshot,
  CheckInfo,
  ReviewComment,
  InlineReviewComment,
} from "@/tauri/types";
import { cn } from "@/lib/utils";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useTitlebarOverlay } from "@/hooks/use-gui-chrome";

interface Props {
  workspace: WorkspaceSnapshot;
  activeTab: RightPanelTab;
}

type ChecksRollup = "pending" | "success" | "failure" | "none";

function rollupChecks(checks: CheckInfo[]): ChecksRollup {
  if (checks.length === 0) return "none";
  let allDone = true;
  for (const c of checks) {
    const s = (c.conclusion ?? c.status).toLowerCase();
    if (s === "failure" || s === "fail") return "failure";
    if (s !== "success" && s !== "pass") allDone = false;
  }
  return allDone ? "success" : "pending";
}

// Reads cached PR query data (enabled:false) so the Review tab can show
// a check + comment-count badge without triggering its own fetches.
// Once the panel mounts, its polling keeps these values fresh in cache.
function ReviewTabBadge({
  workspaceId,
  prNumber,
}: {
  workspaceId: string;
  prNumber: number | null;
}) {
  const checksData = useQuery<CheckInfo[]>({
    queryKey: ["pr", "checks", workspaceId, prNumber] as const,
    enabled: false,
  }).data ?? [];
  const reviewsData = useQuery<ReviewComment[]>({
    queryKey: ["pr", "reviews", workspaceId, prNumber] as const,
    enabled: false,
  }).data ?? [];
  const inlineData = useQuery<InlineReviewComment[]>({
    queryKey: ["pr", "inline", workspaceId, prNumber] as const,
    enabled: false,
  }).data ?? [];

  const commentCount =
    reviewsData.length + inlineData.filter((c) => !c.in_reply_to_id).length;
  const status = rollupChecks(checksData);

  return (
    <>
      {commentCount > 0 && (
        <span className="ml-1 text-[10px] tabular-nums text-muted-foreground/60">
          {commentCount}
        </span>
      )}
      {status === "pending" && (
        <Loader2 className="ml-1 size-3 animate-spin text-status-working" />
      )}
      {status === "success" && (
        <Check className="ml-1 size-3 text-status-open" />
      )}
      {status === "failure" && (
        <X className="ml-1 size-3 text-status-attention" />
      )}
    </>
  );
}

// Tab styling mirrors the main tab-bar's active/inactive treatment so
// the right-panel header reads as a continuation of the tab strip, not
// a separate slab. Active = card fill + foreground text; inactive =
// muted text with a hairline divider, hover lifts toward foreground.
const TAB_TRIGGER_CLS = cn(
  "px-3 !h-full !py-0 !m-0 text-xs !rounded-none !border-transparent !shadow-none after:!hidden",
  "data-[state=active]:!bg-card data-[state=active]:!text-foreground",
  "data-[state=inactive]:!text-muted-foreground/70 data-[state=inactive]:!border-r data-[state=inactive]:!border-r-border/40",
  "data-[state=inactive]:hover:!text-muted-foreground data-[state=inactive]:hover:!bg-muted/20",
);

// #127: memo is effective because setAppState performs structural sharing — the
// `workspace` snapshot keeps a stable ref across backend ticks that don't change
// it, and `activeTab` is a primitive, so shallow compare skips re-renders.
export const RightPanel = memo(function RightPanel({ workspace, activeTab }: Props) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  // Two independent reasons the tab row may need to start 40px down, and
  // both only exist while the titlebar is a floating overlay:
  //   1. the native window controls island pinned to the top-right corner
  //      (desktop only — the web client renders none), and
  //   2. the overlay's own drag layer, which is skipped on the web client
  //      precisely because it has nothing to drag (see title-bar.tsx).
  // With legacy chrome the in-flow `h-9` bar already pushes this panel
  // down, so any clearance here would be a blank band above the tabs.
  const titlebarOverlay = useTitlebarOverlay();
  const workspaceWorkflow = useWorkspaceWorkflow(workspace);
  const {
    tasks: activeChatTasks,
    updatedAt: tasksUpdatedAt,
    streaming: tasksThreadStreaming = false,
  } = useActiveChatTasks(workspace);
  const tasksSnapshot =
    activeChatTasks && activeChatTasks.tasks.length > 0 ? activeChatTasks : null;
  // The blinking tab dot is a live affordance: gate it on the thread
  // actually running, not on the durable snapshot. A plan the provider
  // left with an `in_progress` row survives the turn (and a restart, via
  // hydrate-replay) and would otherwise blink forever.
  const tasksRunning =
    tasksThreadStreaming &&
    (tasksSnapshot?.tasks.some((task) => task.status === "in_progress") ?? false);
  // The Orchestration tab appears only once a run is approved (design:
  // the approval card in the thread owns the pending_approval state; the
  // panel would just duplicate the planned phases as "queued").
  const workflowRun =
    workspaceWorkflow.run != null &&
    workspaceWorkflow.run.status !== "pending_approval"
      ? workspaceWorkflow.run
      : null;
  const workflowThreadId = workflowRun != null ? workspaceWorkflow.threadId : null;

  const handleTabChange = (value: string) => {
    setRightPanelTab(workspace.workspace_id, value as RightPanelTab);
  };

  return (
    <div className="flex h-full min-h-0 flex-col border-l border-border bg-background overflow-hidden">
      <Tabs
        value={activeTab}
        onValueChange={handleTabChange}
        className="flex h-full flex-col"
      >
        <div
          data-testid="right-panel-tabs-header"
          className={cn(
            "flex h-[45px] shrink-0 items-center border-b border-border",
            titlebarOverlay && !isRemoteClient() && "mt-10",
          )}
        >
          <TabsList variant="line" className="!h-full !p-0 gap-0 flex-1">
            <TabsTrigger value="files" className={TAB_TRIGGER_CLS}>
              Files
            </TabsTrigger>
            <TabsTrigger value="changes" className={TAB_TRIGGER_CLS}>
              Changes
              {workspace.git_changed_files > 0 && (
                <span className="ml-1 text-[10px] tabular-nums text-muted-foreground/60">
                  {workspace.git_changed_files}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="review" className={TAB_TRIGGER_CLS}>
              Review
              {workspace.pr_number != null && (
                <ReviewTabBadge
                  workspaceId={workspace.workspace_id}
                  prNumber={workspace.pr_number}
                />
              )}
            </TabsTrigger>
            {workflowRun != null && (
              <TabsTrigger
                value="orchestration"
                className={TAB_TRIGGER_CLS}
                data-testid="orchestration-tab"
              >
                Orchestration
                {workflowRun.status === "running" && (
                  <span
                    className="cm-blink ml-1.5 h-1.5 w-1.5 rounded-full bg-status-working"
                    aria-hidden
                  />
                )}
              </TabsTrigger>
            )}
            {tasksSnapshot && (
              <TabsTrigger
                value="tasks"
                className={TAB_TRIGGER_CLS}
                data-testid="tasks-tab"
              >
                Tasks
                <span className="ml-1 text-[10px] tabular-nums text-muted-foreground/60">
                  {tasksSnapshot.tasks.filter((task) => task.status === "completed").length}/
                  {tasksSnapshot.tasks.length}
                </span>
                {/* Progress stays readable from any tab: a live dot marks
                    an in-flight step until the user is actually looking. */}
                {tasksRunning && activeTab !== "tasks" && (
                  <span
                    data-testid="tasks-live-dot"
                    className="cm-blink ml-1.5 h-1.5 w-1.5 rounded-full bg-status-working"
                    aria-hidden
                  />
                )}
              </TabsTrigger>
            )}
          </TabsList>
        </div>
        <TabsContent value="files" className="flex-1 overflow-hidden">
          <FileTreePanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="changes" className="flex-1 overflow-hidden">
          <ChangesPanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="review" className="flex-1 overflow-hidden">
          <ReviewPanel workspace={workspace} />
        </TabsContent>
        {workflowRun != null && (
          <TabsContent value="orchestration" className="flex-1 overflow-hidden">
            <OrchestrationPanel
              workspace={workspace}
              run={workflowRun}
              threadId={workflowThreadId}
            />
          </TabsContent>
        )}
        {tasksSnapshot && (
          <TabsContent value="tasks" className="flex-1 overflow-hidden">
            <TasksPanel snapshot={tasksSnapshot} updatedAt={tasksUpdatedAt} />
          </TabsContent>
        )}
      </Tabs>
    </div>
  );
});
