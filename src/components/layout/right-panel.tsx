import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Check, Loader2, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useUIStore, type RightPanelTab } from "@/stores/ui-store";
import { ChangesPanel } from "@/components/workspace/changes-panel";
import { FileTreePanel } from "@/components/workspace/file-tree-panel";
import { ReviewPanel } from "@/components/workspace/review-panel";
import type {
  WorkspaceSnapshot,
  CheckInfo,
  ReviewComment,
  InlineReviewComment,
} from "@/tauri/types";
import { cn } from "@/lib/utils";

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

/**
 * Tab-trigger badge for the Review tab. Subscribes to the same React
 * Query keys the ReviewPanel uses but with `enabled: false` so the tab
 * trigger only reads cached data — no fetches start until the user
 * actually opens the Review panel. Once the panel has been opened
 * (priming the cache), the tab keeps showing live status as the panel
 * polls in the background, even after the user switches tabs.
 */
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
      <span className="ml-1 text-[10px] tabular-nums text-muted-foreground">
        {commentCount}
      </span>
      {status === "pending" && (
        <Loader2 className="ml-1 size-3 animate-spin text-amber-500" />
      )}
      {status === "success" && (
        <Check className="ml-1 size-3 text-emerald-500" />
      )}
      {status === "failure" && (
        <X className="ml-1 size-3 text-red-500" />
      )}
    </>
  );
}

const TAB_TRIGGER_CLS = cn(
  "px-3 !h-full !py-0 !m-0 text-xs !rounded-none !border-transparent !shadow-none after:!hidden",
  "data-[state=active]:!bg-card data-[state=active]:!text-foreground",
  "data-[state=inactive]:!text-muted-foreground/70 data-[state=inactive]:!border-r data-[state=inactive]:!border-r-border/40",
  "data-[state=inactive]:hover:!text-muted-foreground data-[state=inactive]:hover:!bg-muted/20",
);

export function RightPanel({ workspace, activeTab }: Props) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);

  const handleTabChange = (value: string) => {
    setRightPanelTab(workspace.workspace_id, value as RightPanelTab);
  };

  const handleClose = () => {
    setRightPanelTab(workspace.workspace_id, null);
  };

  return (
    <div className="flex h-full min-h-0 flex-col border-l border-border/50 bg-background overflow-hidden">
      <Tabs
        value={activeTab}
        onValueChange={handleTabChange}
        className="flex h-full flex-col"
      >
        <div className="flex items-center h-[52px] shrink-0 border-b border-border/50">
          <TabsList variant="line" className="!h-full !p-0 gap-0 flex-1">
            <TabsTrigger value="changes" className={TAB_TRIGGER_CLS}>
              Changes
              {workspace.git_changed_files > 0 && (
                <span className="ml-1 text-[10px] tabular-nums text-muted-foreground">
                  {workspace.git_changed_files}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="files" className={TAB_TRIGGER_CLS}>
              Files
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
          </TabsList>
          <Button
            variant="ghost"
            size="icon-xs"
            className="shrink-0"
            onClick={handleClose}
            title="Close panel"
          >
            <X className="h-3 w-3" />
          </Button>
        </div>
        <TabsContent value="changes" className="flex-1 overflow-hidden">
          <ChangesPanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="files" className="flex-1 overflow-hidden">
          <FileTreePanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="review" className="flex-1 overflow-hidden">
          <ReviewPanel workspace={workspace} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
