import { useState, useEffect } from "react";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { ProjectImageDialog } from "@/components/overlays/project-image-dialog";
import { PROJECT_COLORS } from "./project-appearance-menu";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ChevronRight, Plus, Check, Loader2, AlertCircle, FolderOpen, Clipboard, Home, Image as ImageIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  dbGetUiState,
  dbSetUiState,
  archiveWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  revealInFileManager,
  createEmptyWorkspace,
  agentChatCreatePane,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import type { WorkspaceSnapshot, PendingWorkspace } from "@/tauri/types";

interface Props {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
  onWorkspaceDragStart?: (workspaceId: string, projectPath: string | null) => (e: React.DragEvent) => void;
  onProjectDragStart?: (e: React.DragEvent) => void;
  dragStateId?: string | null;
  pendingWorkspaces?: PendingWorkspace[];
  /** Workspaces lifted into the LIVE section ("gather on top" mode) — skipped
   *  here so the group shows only its idle remainder. Their `data-ws-index`
   *  slots are still counted (indices stay in full-group space) so drag
   *  reorder of the remaining rows lands correctly. */
  hiddenWorkspaceIds?: Set<string>;
}

export function SidebarProjectGroup({
  projectName,
  projectPath,
  workspaces,
  activeWorkspaceId,
  onWorkspaceDragStart,
  onProjectDragStart,
  dragStateId,
  pendingWorkspaces = [],
  hiddenWorkspaceIds,
}: Props) {
  const { getKeysForAction } = useResolvedKeybinds();
  const newWsKeys = getKeysForAction("newWorkspaceInProject");
  const [collapsed, setCollapsed] = useState(false);
  const [customColor, setCustomColor] = useState<string | null>(null);
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  // Cache-bust token for the derived favicon. Bumped on every save so a site
  // that changed its favicon is re-fetched instead of served stale from cache.
  const [imageVersion, setImageVersion] = useState<string | null>(null);
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const expandProjectRequest = useUIStore((s) => s.expandProjectRequest);
  const clearExpandProjectRequest = useUIStore((s) => s.clearExpandProjectRequest);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );

  useEffect(() => {
    dbGetUiState(`collapsed:project:${projectPath}`).then((val) => {
      if (val === "true") setCollapsed(true);
    }).catch(() => {});
    dbGetUiState(`project.color:${projectPath}`).then((val) => {
      if (val) setCustomColor(val);
    }).catch(() => {});
    dbGetUiState(`project.image:${projectPath}`).then((val) => {
      if (val) setImageUrl(val);
    }).catch(() => {});
    dbGetUiState(`project.image.v:${projectPath}`).then((val) => {
      if (val) setImageVersion(val);
    }).catch(() => {});
  }, [projectPath]);

  // Consume an external expand request (e.g. the "Needs you" strip jumping to
  // a blocked row inside this collapsed group). Expand + persist when it
  // targets this group, then clear the one-shot request so it fires once.
  useEffect(() => {
    if (expandProjectRequest !== projectPath) return;
    if (collapsed) {
      setCollapsed(false);
      dbSetUiState(`collapsed:project:${projectPath}`, "false").catch(
        console.error,
      );
    }
    clearExpandProjectRequest(projectPath);
  }, [expandProjectRequest, projectPath, collapsed, clearExpandProjectRequest]);

  const handleToggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    dbSetUiState(`collapsed:project:${projectPath}`, String(next)).catch(console.error);
  };

  const handleColorChange = (color: string | null) => {
    setCustomColor(color);
    if (color) {
      dbSetUiState(`project.color:${projectPath}`, color).catch(console.error);
    } else {
      dbSetUiState(`project.color:${projectPath}`, "").catch(console.error);
    }
  };

  const [showImageDialog, setShowImageDialog] = useState(false);

  const handleSaveImage = (next: string | null) => {
    if (!next) {
      setImageUrl(null);
      setImageVersion(null);
      dbSetUiState(`project.image:${projectPath}`, "").catch(console.error);
      dbSetUiState(`project.image.v:${projectPath}`, "").catch(console.error);
    } else {
      // New token on every save forces the favicon to refresh, so re-adding a
      // site whose favicon changed picks up the new icon instead of the cached one.
      const version = String(Date.now());
      setImageUrl(next);
      setImageVersion(version);
      dbSetUiState(`project.image:${projectPath}`, next).catch(console.error);
      dbSetUiState(`project.image.v:${projectPath}`, version).catch(console.error);
    }
  };

  const [showCloseDialog, setShowCloseDialog] = useState(false);

  const handleRevealInFileManager = () => {
    revealInFileManager(projectPath).catch(console.error);
  };

  const handleCopyPath = () => {
    navigator.clipboard.writeText(projectPath).catch(console.error);
  };

  const handlePlusClick = async (e: React.MouseEvent) => {
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
      await activateWorkspaceInteraction(wsId);
      await agentChatCreatePane(wsId, null, projectPath);
    } catch (err) {
      console.error("[sidebar] failed to open chat pane:", err);
      setShowNewWorkspaceDialog(true, projectPath);
    }
  };

  // Archive the whole group — non-destructive: every workspace becomes an
  // archive entry (files/branches/worktrees untouched) and can be restored
  // individually from Settings → Archive. Attach-in-place and remote
  // (host-backed) members can't be archived (the backend refuses), so they
  // get the plain non-destructive close instead — mixed groups still fully
  // clear, like the old Close Project. Worktrees go first so the root
  // anchors the group until the end. The awaits are deliberately
  // sequential: each archive/close can run teardown scripts and git
  // subprocesses in the same repo, and firing them concurrently risks
  // racing those (lock contention, half-torn-down worktrees).
  const handleArchiveProject = async () => {
    const ordered = [
      ...workspaces.filter((ws) => ws.worktree_path),
      ...workspaces.filter((ws) => !ws.worktree_path),
    ];
    const failures: string[] = [];
    for (const ws of ordered) {
      const isAttachOrRemote = ws.attach_only === true || ws.host_id != null;
      try {
        if (isAttachOrRemote) {
          if (ws.worktree_path) {
            await closeWorkspaceWithWorktree(
              ws.workspace_id,
              false,
              false,
              false,
            );
          } else {
            await closeWorkspace(ws.workspace_id, false);
          }
        } else {
          await archiveWorkspace(ws.workspace_id);
        }
      } catch (err) {
        console.error("[sidebar] archive failed:", err);
        failures.push(ws.title);
      }
    }
    if (failures.length > 0) {
      toast.error("Some workspaces could not be archived", {
        description: failures.join(", "),
      });
    } else {
      // Chat drafts are only cleared once every member is actually gone
      // — clearing up front wiped drafts even when archiving failed.
      const homeDir = useAppStore.getState().homeDir;
      useChatDraftStore.getState().clearDraftsForProject(projectPath, homeDir);
    }
    setShowCloseDialog(false);
  };

  const homeDir = useHomeDir();
  const isHomeGroup = projectName === "Home" && projectPath === homeDir;

  // In "gather on top" mode the header counts only the idle remainder shown
  // here (the live rows live in the LIVE section above).
  const visibleCount = hiddenWorkspaceIds
    ? workspaces.filter((ws) => !hiddenWorkspaceIds.has(ws.workspace_id)).length
    : workspaces.length;

  return (
    <div className="pt-2.5">
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            className="group/proj relative flex items-center mx-1.5 pl-1.5 pr-2 py-1 text-sm font-medium hover:bg-muted/40 transition-colors cursor-pointer rounded-lg"
            draggable={!!onProjectDragStart}
            onDragStart={onProjectDragStart}
            data-project-header-path={projectPath}
            onClick={handleToggle}
          >
            {/* Round avatar — image if set, else muted letter, else
                colored letter when user picked a color. The Home group
                renders a Home glyph instead of a letter. */}
            {isHomeGroup ? (
              <div className="size-5 rounded-full border border-border bg-muted flex items-center justify-center shrink-0 mr-2.5">
                <Home className="h-3 w-3 text-muted-foreground" />
              </div>
            ) : (
              <ProjectAvatar
                name={projectName}
                color={customColor}
                imageUrl={imageUrl}
                cacheBust={imageVersion}
                size="md"
                shape="circle"
                className="mr-2.5"
              />
            )}

            <span className="flex-1 min-w-0 truncate text-foreground/90 text-[13px]">
              {projectName}
            </span>

            {/* Count — visible at rest, fades on hover so the + can
                take its slot without ever colliding. */}
            <span className="text-[11px] text-muted-foreground/60 tabular-nums font-normal mr-1 transition-opacity group-hover/proj:opacity-0">
              {visibleCount}
            </span>

            {/* + button — hover-reveal */}
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="absolute right-7 opacity-0 group-hover/proj:opacity-100 transition-opacity"
                  aria-label="New workspace"
                  onClick={handlePlusClick}
                >
                  <Plus className="h-3.5 w-3.5 text-muted-foreground" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                {enableAgentChat ? "New workspace · Shift+click for CLI" : "New workspace"}
                {newWsKeys ? ` · ${newWsKeys}` : ""}
              </TooltipContent>
            </Tooltip>

            {/* Chevron stays visible as the only state cue */}
            <ChevronRight
              className={cn(
                "h-3.5 w-3.5 text-muted-foreground/50 transition-transform duration-150 shrink-0",
                !collapsed && "rotate-90",
              )}
            />
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onClick={handleRevealInFileManager}>
            <FolderOpen className="mr-2 h-3.5 w-3.5" />
            Open in File Manager
          </ContextMenuItem>
          <ContextMenuItem onClick={handleCopyPath}>
            <Clipboard className="mr-2 h-3.5 w-3.5" />
            Copy Path
          </ContextMenuItem>
          <ContextMenuItem onClick={() => setShowImageDialog(true)}>
            <ImageIcon className="mr-2 h-3.5 w-3.5" />
            {imageUrl ? "Change image…" : "Set image…"}
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>Change Color</ContextMenuSubTrigger>
            <ContextMenuSubContent className="w-36">
              <ContextMenuItem onClick={() => handleColorChange(null)}>
                <span className="size-3.5 rounded-full border border-border bg-background shrink-0 mr-2" />
                Default
                {!customColor && <Check className="ml-auto h-3.5 w-3.5" />}
              </ContextMenuItem>
              {PROJECT_COLORS.map((color) => (
                <ContextMenuItem key={color.value} onClick={() => handleColorChange(color.value)}>
                  <span
                    className="size-3.5 rounded-full shrink-0 mr-2 border border-border/50"
                    style={{ backgroundColor: color.value }}
                  />
                  {color.name}
                  {customColor === color.value && <Check className="ml-auto h-3.5 w-3.5" />}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
          <ContextMenuItem onClick={() => setShowCloseDialog(true)}>
            Archive Project
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <ProjectImageDialog
        open={showImageDialog}
        onOpenChange={setShowImageDialog}
        projectName={projectName}
        initialValue={imageUrl}
        onSave={handleSaveImage}
      />

      <Dialog open={showCloseDialog} onOpenChange={setShowCloseDialog}>
        <DialogContent showCloseButton={false} className="max-w-[340px]">
          <DialogHeader>
            <DialogTitle className="text-sm">
              Archive project &ldquo;{projectName}&rdquo;?
            </DialogTitle>
            <DialogDescription>
              This will archive {workspaces.length} workspace{workspaces.length !== 1 ? "s" : ""} and
              stop the active terminals in this project. Files, branches, and
              worktrees stay on disk — everything can be restored from
              Settings → Archive.
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2 pt-1">
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-3 text-xs"
              onClick={() => setShowCloseDialog(false)}
            >
              Cancel
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className="h-7 px-3 text-xs"
              onClick={() => void handleArchiveProject()}
            >
              Archive Project
            </Button>
          </div>
        </DialogContent>
      </Dialog>

      {!collapsed && workspaces.map((ws, idx) =>
        // Hidden (lifted to the LIVE section) rows render nothing, but `idx`
        // still advances so the remaining rows keep their full-group
        // `data-ws-index` — drag reorder measures in that same index space.
        hiddenWorkspaceIds?.has(ws.workspace_id) ? null : (
          <div
            key={ws.workspace_id}
            data-ws-id={ws.workspace_id}
            data-ws-index={idx}
            draggable={!!onWorkspaceDragStart}
            onDragStart={onWorkspaceDragStart?.(ws.workspace_id, projectPath)}
            className={dragStateId === ws.workspace_id ? "opacity-40" : ""}
          >
            <SidebarWorkspaceRow
              workspace={ws}
              isActive={ws.workspace_id === activeWorkspaceId}
            />
          </div>
        ),
      )}

      {!collapsed && pendingWorkspaces.map((pw) => (
        <div
          key={pw.id}
          className={cn(
            "flex items-center gap-2.5 px-3 py-2 pl-[2.75rem] text-sm",
            pw.status === "failed" ? "opacity-60" : "opacity-70 animate-pulse",
          )}
        >
          {pw.status === "creating" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground shrink-0" />
          ) : (
            <AlertCircle className="h-3.5 w-3.5 text-destructive shrink-0" />
          )}
          <span className="truncate text-muted-foreground text-xs">
            {pw.status === "failed" ? pw.errorMessage || "Failed" : pw.name}
          </span>
        </div>
      ))}
    </div>
  );
}
