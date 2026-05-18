import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
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

export function RightPanel({ workspace, activeTab }: Props) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);

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
        <div className="flex items-center h-[45px] shrink-0 border-b border-border">
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
      </Tabs>
    </div>
  );
}
