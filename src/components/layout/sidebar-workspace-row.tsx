import { useEffect, useState } from "react";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuGroup,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import { groupEditors } from "@/lib/editor-groups";
import { EditorIcon } from "@/components/icons/editor-icon";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  AlarmClock,
  AlarmClockOff,
  Archive,
  Check,
  Clock,
  Copy,
  Laptop,
  GitBranch,
  AlertTriangle,
  BellOff,
  Cloud,
  Loader2,
  Mail,
  MonitorSmartphone,
  Pencil,
  Pin,
  PinOff,
  RotateCcw,
  Server,
  Terminal,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import {
  MENU_ROW,
  MENU_ROW_DESTRUCTIVE,
  MENU_ROW_META,
  MENU_ROW_TWO_LINE_COMPACT,
  MENU_SECTION_LABEL,
  MENU_SEPARATOR,
  MenuKeycap,
} from "@/components/ui/menu-chrome";
import { useProjectAppearance } from "./use-project-appearance";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrStatusIcon, humanizePrState } from "@/components/github/pr-status-icon";
import {
  archiveWorkspace,
  checkoutDefaultBranchInWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  renameWorkspace,
  unarchiveWorkspace,
  setWorkspaceMuted,
  setWorkspacePinned,
  detectEditors,
  openInEditor,
  runWorkspaceSetup,
  workspacePullBack,
  workspacePushToHost,
  type HostView,
} from "@/tauri/commands";
import { useHosts } from "@/stores/hosts-store";
import {
  ConfirmPushDialog,
  shouldSkipPushConfirm,
} from "@/components/overlays/confirm-push-dialog";
import type { WorkspaceSnapshot, EditorInfo, ActivePaneStatus } from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import {
  useTunnelStatusStore,
  tunnelStatusKind,
} from "@/stores/tunnel-status-store";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { getWorkspaceStatus } from "@/lib/pane-status";
import {
  useSidebarDensityStore,
  formatElapsed,
  permissionBlockerText,
  isReviewExpanded,
  isRetiredPr,
  SETTLED_FADE_MS,
} from "@/stores/sidebar-density-store";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import {
  useSettingsStore,
  selectWorkingIndicator,
  selectWorkingIndicatorColor,
} from "@/stores/settings-store";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { WorkingIndicator } from "@/components/ui/working-indicator";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";
import { toast } from "@/lib/toast";
import { useForceDelete } from "@/hooks/use-force-delete";
import { useDefaultBranch } from "./default-branch-cache";
import { computeSnoozePresets } from "./sidebar-snooze";

/** Attach-in-place and remote (pushed-to-host) workspaces can't be
 *  archived — the backend refuses — so their removal affordance is the
 *  old non-destructive close instead. */
function isAttachOrRemoteWorkspace(workspace: WorkspaceSnapshot): boolean {
  return workspace.attach_only === true || workspace.host_id != null;
}

interface Props {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
  /** In "gather on top" mode a live row is lifted out of its project group
   *  into the LIVE section; this leading chip keeps its project origin
   *  visible. Absent (the default) inside the normal project tree. */
  projectChip?: { name: string; color: string | null };
}

/** Destructive delete confirmation for deletable worktrees. Archiving
 *  replaced the old hide/close paths, so this dialog only ever opens for
 *  a worktree the user explicitly asked to delete (shift-click on the
 *  archive button, or the context menu's "Delete Worktree…").
 *
 *  Escalation state machine (shared `useForceDelete` hook): the first
 *  Delete issues `closeWorkspaceWithWorktree(..., forceDelete: false)`.
 *  If the backend rejects with a dirty-worktree message matching
 *  `USE_FORCE_PATTERN`, the dialog stays open, shows that message
 *  verbatim in the warning box, and the button flips to "Force delete",
 *  which reissues the same call with `forceDelete: true`. Any other
 *  rejection surfaces as an error toast and closes the dialog. */
export function DeleteWorktreeDialog({
  workspace,
  open,
  onOpenChange,
}: {
  workspace: WorkspaceSnapshot;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [deleteBranch, setDeleteBranch] = useState(true);

  const hasUnpushed = workspace.git_ahead > 0;
  const hasUncommitted = workspace.git_changed_files > 0;
  const hasWarnings = hasUnpushed || hasUncommitted;

  const warningMessage = hasUnpushed && hasUncommitted
    ? "Has uncommitted changes and unpushed commits"
    : hasUncommitted
      ? "Has uncommitted changes"
      : hasUnpushed
        ? "Has unpushed commits"
        : null;

  const { forceMessage, confirm, reset } = useForceDelete({
    run: (force) =>
      closeWorkspaceWithWorktree(
        workspace.workspace_id,
        true,
        deleteBranch,
        force,
      ),
    onDone: () => handleOpenChange(false),
    onError: (message) => {
      toast.error("Delete failed", { description: message });
      handleOpenChange(false);
    },
  });

  function handleOpenChange(next: boolean) {
    if (!next) {
      // Reset per-open state so a re-open starts un-escalated.
      reset();
      setDeleteBranch(true);
    }
    onOpenChange(next);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-[340px]">
        <DialogHeader>
          <DialogTitle className="text-sm">
            Delete worktree &ldquo;{workspace.title}&rdquo;?
          </DialogTitle>
          <DialogDescription>
            Permanently removes the worktree directory from disk. To keep
            the files, archive the workspace instead.
          </DialogDescription>
        </DialogHeader>

        {(forceMessage !== null || hasWarnings) && (
          <div className="flex items-center gap-2 rounded-md border border-status-working/20 bg-status-working/10 px-2.5 py-1.5 text-xs text-status-working">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            {forceMessage ?? warningMessage}
          </div>
        )}

        <label className="flex items-center gap-2 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={deleteBranch}
            onChange={(e) => setDeleteBranch(e.target.checked)}
            className="rounded border-border"
          />
          <span className="text-xs text-muted-foreground">
            Also delete local branch
          </span>
        </label>

        <div className="flex justify-end gap-2 pt-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={() => handleOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            variant="destructive"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={() => void confirm()}
          >
            {forceMessage !== null ? "Force delete" : "Delete"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Optional Settle / Un-settle entry surfaced at the top of the workspace
 *  context menu — the sidebar inbox's card and settled-row shapes pass it so
 *  the right-click menu mirrors their hover affordance. */
export interface SettleMenuAction {
  kind: "settle" | "unsettle";
  onAction: () => void;
}

/** Optional Snooze / Wake entry for the workspace context menu, mirroring
 *  {@link SettleMenuAction}. A snoozed row passes `kind: "wake"`; an active
 *  card passes `kind: "snooze"`. */
export interface SnoozeMenuAction {
  kind: "snooze" | "wake";
  /** Whether deferral is on offer at all — the caller's settle/snooze
   *  guardrail, which is the only thing that may withhold the submenu. Read
   *  only for `kind: "snooze"`.
   *
   *  This replaced "an empty preset array means not offered". Presets are now
   *  resolved when the submenu opens rather than by whoever renders the list,
   *  so there is no array left to be empty and the guardrail has to say so in
   *  its own words. */
  offered: boolean;
  onSnooze: (until: number) => void;
  onWake: () => void;
}

/** The "Snooze until…" submenu.
 *
 *  Its own component purely so the wake times are resolved at the right moment:
 *  Radix does not render a context menu's content until the menu opens, so this
 *  mounts on open and `useState`'s initialiser runs exactly once, right then.
 *  Resolving them where the *row* renders — which is what passing a preset
 *  array down did — meant "In 1 hour" could be up to a full coarse tick (~30s)
 *  in the past by the time it was clicked, and it made every row in the sidebar
 *  re-render on every tick to keep a menu current that nobody had opened. */
function SnoozeUntilSubmenu({
  onSnooze,
}: {
  onSnooze: (until: number) => void;
}) {
  const [presets] = useState(() => computeSnoozePresets(Date.now()));
  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger className={MENU_ROW}>
        <AlarmClock />
        <span className="flex-1">Snooze until…</span>
      </ContextMenuSubTrigger>
      <ContextMenuSubContent {...SUBMENU_CASCADE} className="w-[246px]">
        {presets.map((preset) => (
          <ContextMenuItem
            key={preset.id}
            className={MENU_ROW}
            onClick={() => onSnooze(preset.at)}
          >
            <Clock />
            <span className="flex-1">{preset.label}</span>
            {/* The relative label says how far away a wake is and never when:
                "Next week" is a deferral the user would have to guess the end
                of. */}
            <span className={cn(MENU_ROW_META, "tabular-nums")}>
              {preset.whenLabel}
            </span>
          </ContextMenuItem>
        ))}
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}

/**
 * A submenu cascades off its trigger row with its FIRST ITEM level with that
 * row, not with the panel's top edge — otherwise the entry the user is
 * reaching for sits a row lower than the one they pointed at. The negative
 * align offset backs the panel up by its own 6px padding; the negative side
 * offset overlaps the parent by 4px so the pointer never crosses a gap (and
 * so never falls through to a sibling row on the way across).
 */
const SUBMENU_CASCADE = { sideOffset: -4, alignOffset: -6 } as const;

/** Project identity for the menu's header strip. Kept optional: the legacy
 *  tree row renders this menu without a resolved project. */
export interface MenuProject {
  name: string;
  path: string;
}

/** The avatar in the menu header, resolving the project's chosen colour or
 *  image. Its own component so the appearance hook runs unconditionally. */
function MenuProjectAvatar({ project }: { project: MenuProject }) {
  const appearance = useProjectAppearance(project.path);
  return (
    <ProjectAvatar
      name={project.name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      shape="square"
      className="size-[23px] rounded-[7px] text-[10.5px] font-semibold"
    />
  );
}

/**
 * Identity strip at the top of the workspace menu.
 *
 * A right-click menu is the one surface with no visual tie back to the row it
 * came from — it is portalled to the pointer, and every entry in it is
 * destructive-adjacent enough that "which workspace is this?" is a fair
 * question. The strip answers it (workspace, project, and how much
 * uncommitted work is at stake) and is deliberately not a menuitem: it is
 * context, so keyboard navigation skips straight past it to Pin.
 */
function WorkspaceMenuHeader({
  workspace,
  project,
}: {
  workspace: WorkspaceSnapshot;
  project: MenuProject | null;
}) {
  const additions = workspace.git_additions ?? 0;
  const deletions = workspace.git_deletions ?? 0;
  const hasStats = additions > 0 || deletions > 0;

  return (
    <div className="mb-1 flex items-center gap-2.5 border-b border-border/70 px-2 pt-2 pb-2.5">
      {project ? (
        <MenuProjectAvatar project={project} />
      ) : (
        <ProjectAvatar
          name={workspace.title}
          shape="square"
          className="size-[23px] rounded-[7px] text-[10.5px] font-semibold"
        />
      )}
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-[12.5px] font-semibold tracking-[-0.01em] text-foreground">
          {workspace.title}
        </span>
        {project && (
          <span className="truncate font-mono text-[9.5px] text-muted-foreground/70">
            {project.name}
          </span>
        )}
      </span>
      {hasStats && (
        <span className="flex shrink-0 gap-1.5 font-mono text-[9.5px] tabular-nums">
          {additions > 0 && (
            <span className="text-status-open/80">+{additions}</span>
          )}
          {deletions > 0 && (
            <span className="text-status-attention/80">−{deletions}</span>
          )}
        </span>
      )}
    </div>
  );
}

/**
 * The "Move to device…" submenu.
 *
 * Two lines per device rather than a bare name: the question a user asks here
 * is "which machine is that, and can it take this right now?", and a hostname
 * alone answers neither. This device leads the list with an accent check, so
 * the menu always states where the workspace is before offering to move it.
 */
function MoveToDeviceSubmenu({
  hosts,
  onPick,
}: {
  hosts: HostView[];
  onPick: (host: HostView) => void;
}) {
  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger className={MENU_ROW}>
        <MonitorSmartphone />
        <span className="flex-1">Move to device…</span>
      </ContextMenuSubTrigger>
      <ContextMenuSubContent {...SUBMENU_CASCADE} className="w-[246px]">
        {/* Not a menuitem: "this device" is where the workspace already is,
            so there is nothing to select. It is the list's anchor. */}
        <div
          className={cn(
            MENU_ROW_TWO_LINE_COMPACT,
            "flex items-center text-foreground",
          )}
        >
          <MonitorSmartphone className="size-3.5 shrink-0 text-muted-foreground/70" />
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="truncate">This device</span>
          </span>
          <Check className="size-3 shrink-0 text-accent-ember" />
        </div>
        {hosts.length === 0 ? (
          <p className="px-[9px] pt-2 pb-1.5 text-[11px] leading-snug text-muted-foreground/70">
            No other devices signed in.
          </p>
        ) : (
          hosts.map((host) => (
            <ContextMenuItem
              key={host.id}
              className={MENU_ROW_TWO_LINE_COMPACT}
              onClick={() => onPick(host)}
            >
              <Server />
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="truncate">{host.name}</span>
                <span className="truncate font-mono text-[9.5px] text-muted-foreground/70">
                  {host.dirty ? "not synced yet" : host.ssh_target}
                </span>
              </span>
            </ContextMenuItem>
          ))
        )}
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}

export function WorkspaceContextMenuItems({
  workspace,
  project,
  settleAction,
  snoozeAction,
  unreadAction,
  projectMenu,
  onArchiveRequest,
  onDeleteRequest,
  onRequestPushConfirm,
}: {
  workspace: WorkspaceSnapshot;
  /** The project this workspace belongs to, for the menu's header strip.
   *  Optional — the legacy tree row has no resolved project, and the header
   *  degrades to the workspace name alone rather than inventing one. */
  project?: MenuProject;
  settleAction?: SettleMenuAction;
  snoozeAction?: SnoozeMenuAction;
  /** Optional "Mark unread" entry. The caller decides when re-marking is
   *  meaningful — an already-unread card passes nothing rather than offering
   *  an item that would do nothing. */
  unreadAction?: { onMarkUnread: () => void };
  /** Optional project-level section (avatar image + accent color) for the
   *  project this workspace belongs to. Passed in as a slot rather than built
   *  here so this module stays workspace-scoped and free of project-appearance
   *  concerns. */
  projectMenu?: React.ReactNode;
  /** Remove the row non-destructively. For local workspaces this
   *  archives (restorable from Settings → Archive); for attach-in-place
   *  and remote (host-backed) workspaces — which the backend refuses to
   *  archive — it performs the plain close instead. */
  onArchiveRequest: () => void;
  /** Open the destructive delete-worktree dialog. Only rendered for
   *  deletable worktrees (never the primary / protected root). */
  onDeleteRequest: () => void;
  /** Called when the user clicks a host in the "Move to host…"
   *  submenu. Opens the Phase-4 confirmation dialog unless the
   *  user previously set "Don't ask again for this host". */
  onRequestPushConfirm?: (host: HostView) => void;
}) {
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  // Const alias so the discriminant narrowing survives into the preset map's
  // callbacks — TypeScript drops narrowing on a (mutable) parameter binding
  // the moment it is read inside a closure.
  const snooze = snoozeAction;
  const isWorktree = !!workspace.worktree_path;
  const isAttachOrRemote = isAttachOrRemoteWorkspace(workspace);
  // Mirror of the row's `canDelete` gate: destructive deletion is only
  // offered for disposable local worktrees — never the primary /
  // protected root, and never attach-in-place / remote rows (their
  // teardown is a plain close that leaves the host side alive).
  const canDelete =
    isWorktree && workspace.protected !== true && !isAttachOrRemote;
  const defaultBranch = useDefaultBranch(
    workspace.project_root ?? (isWorktree ? null : workspace.cwd),
  );

  const hosts = useHosts();

  useEffect(() => {
    detectEditors().then(setEditors).catch(console.error);
  }, []);

  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );

  const handleMoveToHost = async (host: HostView) => {
    setPushPullInFlight(workspace.workspace_id);
    try {
      const result = await workspacePushToHost(
        workspace.workspace_id,
        host.id,
      );
      if (result.ok) {
        // Push success → offer Undo = pull back. Same machinery
        // the workspace's "Pull back to this device" item runs,
        // wrapped so a misclick within 10s is one tap away from
        // recovery. Data-safety guardrail for Phase 4.
        toast.undoable({
          message: `Pushed to ${host.name}`,
          description: "Tap Undo within 10s to pull it back.",
          onUndo: async () => {
            const undoResult = await workspacePullBack(
              workspace.workspace_id,
            );
            if (undoResult.ok) {
              toast.success(`Pulled back from ${host.name}`);
            } else {
              toast.error("Pull back failed", {
                description: undoResult.message,
              });
            }
          },
        });
      } else {
        toast.error(`Push to ${host.name} failed`, {
          description: result.message,
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Push failed", { description: message });
    } finally {
      setPushPullInFlight(null);
    }
  };

  const handlePullBack = async () => {
    setPushPullInFlight(workspace.workspace_id);
    // Capture the source host id BEFORE the pull clears it on the
    // workspace, so the undo closure knows where to push back to.
    const sourceHostId = workspace.host_id;
    const sourceHost = sourceHostId
      ? hosts.find((h) => h.id === sourceHostId)
      : null;
    try {
      const result = await workspacePullBack(workspace.workspace_id);
      if (result.ok) {
        if (sourceHost) {
          toast.undoable({
            message: "Pulled back to this device",
            description: `From ${sourceHost.name}. Tap Undo within 10s to send it back.`,
            onUndo: async () => {
              const undoResult = await workspacePushToHost(
                workspace.workspace_id,
                sourceHost.id,
              );
              if (undoResult.ok) {
                toast.success(`Pushed back to ${sourceHost.name}`);
              } else {
                toast.error("Push back failed", {
                  description: undoResult.message,
                });
              }
            },
          });
        } else {
          // Source host disappeared (deleted between push and
          // pull) — no undo possible, plain success.
          toast.success("Pulled back to this device", {
            description: result.message,
          });
        }
      } else {
        toast.error("Pull back failed", {
          description: result.message,
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Pull back failed", { description: message });
    } finally {
      setPushPullInFlight(null);
    }
  };

  const handleRename = () => {
    const newTitle = window.prompt("Rename workspace", workspace.title);
    if (newTitle && newTitle !== workspace.title) {
      renameWorkspace(workspace.workspace_id, newTitle).catch(console.error);
    }
  };

  const handleCopyBranch = () => {
    if (workspace.git_branch) {
      navigator.clipboard.writeText(workspace.git_branch).catch(console.error);
    }
  };

  const handleToggleMute = () => {
    setWorkspaceMuted(
      workspace.workspace_id,
      !workspace.notifications_muted,
    ).catch(console.error);
  };

  const handleTogglePinned = async () => {
    const pinned = workspace.pinned_at != null;
    try {
      await setWorkspacePinned(workspace.workspace_id, !pinned);
    } catch (err) {
      toast.error(pinned ? "Unpin failed" : "Pin failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const handleOpenInEditor = (editorId: string) => {
    openInEditor(editorId, workspace.cwd).catch(console.error);
  };

  const handleCheckoutDefault = async () => {
    try {
      const branch = await checkoutDefaultBranchInWorkspace(workspace.workspace_id);
      toast.success(`Switched to ${branch}`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error(`Couldn't switch: ${message}`);
    }
  };

  const showCheckoutDefault = !isWorktree;
  const isOnDefault =
    defaultBranch !== null && workspace.git_branch === defaultBranch;

  return (
    <ContextMenuContent className="w-[274px]">
      <WorkspaceMenuHeader workspace={workspace} project={project ?? null} />
      {/* Section labels rather than bare rules: a divider says "these are
          different", a label says what they are — and the workspace menu is
          long enough that the difference is worth naming. They render through
          the Label slot, so they are not menuitems and arrow keys skip them. */}
      <ContextMenuLabel className={MENU_SECTION_LABEL}>
        Workspace
      </ContextMenuLabel>
      {/* Pin/unpin leads the menu: it is the one entry that is always
          available and always means the same thing, whatever shelf the row is
          on. It also gates the lifecycle block below, so reading it first
          explains why those entries may be missing. */}
      <ContextMenuItem
        className={MENU_ROW}
        onClick={() => void handleTogglePinned()}
      >
        {workspace.pinned_at != null ? <PinOff /> : <Pin />}
        <span className="flex-1">
          {workspace.pinned_at != null ? "Unpin workspace" : "Pin workspace"}
        </span>
      </ContextMenuItem>
      {/* Lifecycle block, hidden entirely while pinned — a pin suppresses
          every park verb, and showing them disabled would only invite the
          click. Ordered so the entries that BRING WORK BACK sit first: a user
          who right-clicked a hidden row is almost always there to undo the
          hiding, and burying "Wake now" under the deferral options would make
          the common case the slower one. */}
      {workspace.pinned_at == null && (
        <>
          {settleAction?.kind === "unsettle" && (
            <ContextMenuItem className={MENU_ROW} onClick={settleAction.onAction}>
              <Undo2 />
              <span className="flex-1">Un-settle workspace</span>
            </ContextMenuItem>
          )}
          {snooze?.kind === "wake" && (
            <ContextMenuItem className={MENU_ROW} onClick={snooze.onWake}>
              <AlarmClockOff />
              <span className="flex-1">Wake now</span>
            </ContextMenuItem>
          )}
          {settleAction?.kind === "settle" && (
            <ContextMenuItem className={MENU_ROW} onClick={settleAction.onAction}>
              <Check />
              <span className="flex-1">Settle workspace</span>
            </ContextMenuItem>
          )}
          {/* `offered` is the guardrail (a working or blocked workspace is
              never deferrable). The wake times themselves belong to
              `SnoozeUntilSubmenu`, which only exists while the menu is open. */}
          {snooze?.kind === "snooze" && snooze.offered && (
            <SnoozeUntilSubmenu onSnooze={snooze.onSnooze} />
          )}
        </>
      )}
      {unreadAction && (
        <ContextMenuItem className={MENU_ROW} onClick={unreadAction.onMarkUnread}>
          <Mail />
          <span className="flex-1">Mark unread</span>
        </ContextMenuItem>
      )}

      <ContextMenuLabel className={cn(MENU_SECTION_LABEL, "mt-1")}>
        Actions
      </ContextMenuLabel>
      <ContextMenuItem className={MENU_ROW} onClick={handleRename}>
        <Pencil />
        <span className="flex-1">Rename workspace</span>
        <MenuKeycap keys="F2" />
      </ContextMenuItem>
      {editors.length === 1 ? (
        <ContextMenuItem
          className={MENU_ROW}
          onClick={() => handleOpenInEditor(editors[0].id)}
        >
          <EditorIcon id={editors[0].id} className="size-3.5 shrink-0" />
          <span className="flex-1">Open in {editors[0].name}</span>
        </ContextMenuItem>
      ) : editors.length > 1 ? (
        (() => {
          const groupedEditors = groupEditors(editors);
          const showGroupLabels = groupedEditors.length > 1;
          return (
            <ContextMenuSub>
              <ContextMenuSubTrigger className={MENU_ROW}>
                <Terminal />
                <span className="flex-1">Open in editor</span>
              </ContextMenuSubTrigger>
              <ContextMenuSubContent {...SUBMENU_CASCADE} className="w-[186px]">
                {groupedEditors.map((group, groupIdx) => (
                  // Same grouping pattern as the title-bar launcher —
                  // render section labels between families when more
                  // than one is installed, skip them when only one
                  // family is present so a "VS Code family" header
                  // doesn't dangle over a single entry.
                  <ContextMenuGroup key={group.id}>
                    {groupIdx > 0 && (
                      <ContextMenuSeparator className={MENU_SEPARATOR} />
                    )}
                    {showGroupLabels && (
                      <ContextMenuLabel className={MENU_SECTION_LABEL}>
                        {group.label}
                      </ContextMenuLabel>
                    )}
                    {group.editors.map((editor) => (
                      <ContextMenuItem
                        key={editor.id}
                        className={MENU_ROW}
                        onClick={() => handleOpenInEditor(editor.id)}
                      >
                        <EditorIcon id={editor.id} className="size-3.5 shrink-0" />
                        <span className="flex-1">{editor.name}</span>
                      </ContextMenuItem>
                    ))}
                  </ContextMenuGroup>
                ))}
              </ContextMenuSubContent>
            </ContextMenuSub>
          );
        })()
      ) : null}
      <ContextMenuItem
        className={MENU_ROW}
        onClick={handleCopyBranch}
        disabled={!workspace.git_branch}
      >
        <Copy />
        <span className="flex-1">Copy branch name</span>
      </ContextMenuItem>
      {showCheckoutDefault && (
        <ContextMenuItem
          className={MENU_ROW}
          onClick={handleCheckoutDefault}
          disabled={isOnDefault}
        >
          <GitBranch />
          <span className="flex-1">Checkout default branch</span>
        </ContextMenuItem>
      )}
      <ContextMenuItem
        className={MENU_ROW}
        onClick={() => runWorkspaceSetup(workspace.workspace_id).catch(console.error)}
      >
        <RotateCcw />
        <span className="flex-1">Re-run Setup</span>
      </ContextMenuItem>
      <ContextMenuItem className={MENU_ROW} onClick={handleToggleMute}>
        <BellOff />
        <span className="flex-1">
          {workspace.notifications_muted
            ? "Unmute notifications"
            : "Mute notifications"}
        </span>
        {/* The label already names the verb; the pill says which way the
            switch is currently thrown, which the label alone cannot. */}
        <span
          aria-hidden
          className={cn(
            "ml-auto flex h-3.5 w-6 shrink-0 items-center rounded-full p-0.5 transition-colors",
            workspace.notifications_muted
              ? "justify-end bg-accent-ember/70"
              : "bg-foreground/15",
          )}
        >
          <span
            className={cn(
              "size-2.5 rounded-full",
              workspace.notifications_muted
                ? "bg-background"
                : "bg-muted-foreground/70",
            )}
          />
        </span>
      </ContextMenuItem>

      <ContextMenuLabel className={cn(MENU_SECTION_LABEL, "mt-1")}>
        Organize
      </ContextMenuLabel>
      {projectMenu}
      {isRemote ? (
        <ContextMenuItem
          className={MENU_ROW}
          onClick={() => void handlePullBack()}
        >
          <MonitorSmartphone />
          <span className="flex-1">Pull back to this device</span>
        </ContextMenuItem>
      ) : (
        // Always a submenu now, hosts or none: the parent row is the answer
        // to "where else could this go?", and a menu that reads
        // "Move to device… (no devices configured)" answers it by removing
        // the affordance. The empty submenu says the same thing without
        // taking the door away.
        <MoveToDeviceSubmenu
          hosts={hosts}
          onPick={(host) => {
            // Phase-4 confirmation gate. If the user clicked
            // "Don't ask again for X" previously, skip the
            // dialog and push immediately — otherwise hoist
            // to the parent to open the confirm modal.
            if (onRequestPushConfirm && !shouldSkipPushConfirm(host.id)) {
              onRequestPushConfirm(host);
            } else {
              void handleMoveToHost(host);
            }
          }}
        />
      )}

      <ContextMenuSeparator className={MENU_SEPARATOR} />
      <ContextMenuItem className={MENU_ROW} onClick={onArchiveRequest}>
        <Archive />
        <span className="flex-1 text-muted-foreground">
          {isAttachOrRemote ? "Close workspace" : "Archive Workspace"}
        </span>
      </ContextMenuItem>
      {canDelete && (
        <ContextMenuItem
          className={MENU_ROW_DESTRUCTIVE}
          onClick={onDeleteRequest}
        >
          <Trash2 />
          <span className="flex-1">Delete Worktree…</span>
        </ContextMenuItem>
      )}
    </ContextMenuContent>
  );
}

export function SidebarWorkspaceRow({ workspace, isActive, projectChip }: Props) {
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  // Phase-4 push confirmation: holds the host the user just picked
  // from the "Move to host…" submenu, so the dialog can render its
  // summary + handle the actual push on confirm.
  const [pendingPushHost, setPendingPushHost] = useState<HostView | null>(
    null,
  );
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );

  const performPushToHost = async (host: HostView) => {
    setPushPullInFlight(workspace.workspace_id);
    try {
      const result = await workspacePushToHost(
        workspace.workspace_id,
        host.id,
      );
      if (result.ok) {
        // Undo = pull back. Same machinery as the workspace's
        // "Pull back to this device" item; gives users a 10s
        // escape hatch (Phase-4 safety guardrail).
        toast.undoable({
          message: `Pushed to ${host.name}`,
          description: "Tap Undo within 10s to pull it back.",
          onUndo: async () => {
            const undoResult = await workspacePullBack(
              workspace.workspace_id,
            );
            if (undoResult.ok) {
              toast.success(`Pulled back from ${host.name}`);
            } else {
              toast.error("Pull back failed", {
                description: undoResult.message,
              });
            }
          },
        });
      } else {
        toast.error(`Push to ${host.name} failed`, {
          description: result.message,
        });
      }
    } catch (err) {
      toast.error("Push failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setPushPullInFlight(null);
    }
  };

  const workspaceStatus: ActivePaneStatus | null = useAppStore((s) => {
    if (!s.appState) return null;
    return getWorkspaceStatus(workspace.surfaces, s.appState.pane_statuses);
  });

  // Configurable working glyph (Settings → Appearance → Agents).
  const indicatorVariant = useSettingsStore(selectWorkingIndicator);
  const indicatorColor = useSettingsStore(selectWorkingIndicatorColor);

  const handleActivate = () => {
    // Paints the selection in this click's own task (the helper clears the
    // active draft too); a transition here would only delay it.
    activateWorkspaceInteraction(workspace.workspace_id).catch(console.error);
  };

  // Archive: remove from sidebar, keep everything on disk. Undo on the
  // toast restores (and re-activates) the workspace via the archive id
  // the backend returned — same 10s escape-hatch machinery as push/pull.
  const handleArchive = async () => {
    try {
      const archiveId = await archiveWorkspace(workspace.workspace_id);
      toast.undoable({
        message: `Archived "${workspace.title}"`,
        description: "Restore anytime from Settings → Archive.",
        onUndo: async () => {
          await unarchiveWorkspace(archiveId);
        },
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Archive failed", { description: message });
    }
  };

  // Non-destructive close for attach-in-place / remote rows: the backend
  // refuses to archive them, so their removal affordance is the old
  // close — worktree rows keep the worktree (removeWorktree=false),
  // plain rows detach. Nothing on the host is torn down; the workspace
  // stays reachable from the Workspaces Overview.
  const handleClose = async () => {
    try {
      if (workspace.worktree_path) {
        await closeWorkspaceWithWorktree(
          workspace.workspace_id,
          false,
          false,
          false,
        );
      } else {
        await closeWorkspace(workspace.workspace_id, false);
      }
      toast.success(
        `Closed "${workspace.title}" — it stays available on its host in the Workspaces Overview`,
      );
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Close failed", { description: message });
    }
  };

  const isPrimary = !workspace.worktree_path;
  // A protected repo root is never destructively deletable from the sidebar
  // (close-only), aligning with the overview's `isRepoRoot` guard. Roots
  // already have a null `worktree_path` (so `isPrimary` covers them), but
  // gating on `protected` too is belt-and-suspenders against a root that
  // somehow carries a worktree_path.
  const isRepoRoot = workspace.protected === true;
  const isAttachOrRemote = isAttachOrRemoteWorkspace(workspace);
  const canDelete = !isPrimary && !isRepoRoot && !isAttachOrRemote;
  const handleArchiveOrClose = isAttachOrRemote ? handleClose : handleArchive;
  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  // SSH tunnel health for this remote workspace (sleep/wake, WiFi flap,
  // circuit-breaker). null for local/healthy → no pill.
  const tunnelKind = tunnelStatusKind(
    useTunnelStatusStore((s) => s.byWorkspace[workspace.workspace_id]),
  );
  const isPushOrPullInFlight = useAppStore(
    (s) => s.workspacePushPullInFlight === workspace.workspace_id,
  );

  // When a worktree workspace has a PR, the leading icon doubles as
  // the PR-state indicator (open=green, merged=purple, closed=red,
  // draft=gray) and becomes a clickable button that opens the PR URL.
  // The PR number rides in the tooltip on hover; there's no trailing
  // pill, since that would duplicate the same signal.
  const isWorktreeRow =
    !isPushOrPullInFlight &&
    !isRemote &&
    !isPrimary;
  // Client-observed work history for the living-row lifecycle: merged-PR
  // retirement + the idle-row "n shipped" tally. Stable reference until the
  // store promotes a new merge, so subscribing here is render-safe.
  const shipped = useSidebarDensityStore(
    (s) => s.workHistory[workspace.workspace_id]?.shipped,
  );
  // A retired PR (its merge already shipped, then newer work was linked)
  // drops back to the plain gray branch icon — the current work title carries
  // the signal now, and the merge lives in the "n shipped" tally instead.
  const prRetired = isRetiredPr(shipped, workspace.pr_number);
  const showWorkspaceIconAsPr =
    isWorktreeRow && !!workspace.pr_state && !prRetired;

  // Phase-4d elapsed-time signal: when an in-flight push/pull
  // crosses 2 seconds, show a small "12s" pill so the user knows
  // the operation is still working. Identical math to the overview
  // row — see workspace-overview-row.tsx LocalRow for the rationale.
  const inFlightStartedAt = useAppStore(
    (s) =>
      s.workspacePushPullInFlight === workspace.workspace_id
        ? s.workspacePushPullStartedAt
        : null,
  );
  const [sidebarElapsedSec, setSidebarElapsedSec] = useState<number | null>(
    null,
  );
  useEffect(() => {
    if (inFlightStartedAt === null) {
      setSidebarElapsedSec(null);
      return;
    }
    const tick = () => {
      const ms = Date.now() - inFlightStartedAt;
      setSidebarElapsedSec(ms < 2_000 ? null : Math.floor(ms / 1_000));
    };
    tick();
    const id = window.setInterval(tick, 1_000);
    return () => window.clearInterval(id);
  }, [inFlightStartedAt]);
  const icon = isPushOrPullInFlight ? (
    <Loader2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground animate-spin" />
  ) : isRemote ? (
    <Cloud className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  ) : isPrimary ? (
    <Laptop className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  ) : showWorkspaceIconAsPr ? (
    <PrStatusIcon state={workspace.pr_state} size={3.5} />
  ) : (
    <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  );

  const prHumanState = humanizePrState(workspace.pr_state);
  const handlePrClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (workspace.pr_url) {
      openUrl(workspace.pr_url).catch(console.error);
    }
  };

  const hasDiff = workspace.git_additions > 0 || workspace.git_deletions > 0;
  const hasAheadBehind = workspace.git_ahead > 0 || workspace.git_behind > 0;
  // Uncommitted work — the user probably cares about the git line even when
  // no agent is running, so a dirty idle row still expands to show it.
  const isDirty = workspace.git_changed_files > 0 || hasDiff;

  // ── State-driven row density ──
  //
  // Replaces the removed Clean/Branch/Detailed setting: the workspace's live
  // agent status + git state decide how many lines the row shows.
  //   working    → 3 lines (spinner + title, activity, git line)
  //   permission → 2 lines (title, blocker) — a "needs you" card
  //   review     → 2 lines (title + chip, "Done …") until seen or ~1h passes
  //   idle       → 1 line, +git line only when dirty / noteworthy
  const observeStatus = useSidebarDensityStore((s) => s.observeStatus);
  const observeWork = useSidebarDensityStore((s) => s.observeWork);
  const markSeen = useSidebarDensityStore((s) => s.markSeen);
  const settledAt = useSidebarDensityStore(
    (s) => s.settledAt[workspace.workspace_id],
  );
  const lastSeenAt = useSidebarDensityStore(
    (s) => s.lastSeenAt[workspace.workspace_id],
  );
  const statusMark = useSidebarDensityStore(
    (s) => s.statusSince[workspace.workspace_id],
  );

  // Observe status transitions (no backend timestamp exists) so elapsed and
  // the settled-✓ fade can be derived client-side. Non-persisted.
  useEffect(() => {
    observeStatus(workspace.workspace_id, workspaceStatus);
  }, [observeStatus, workspace.workspace_id, workspaceStatus]);

  // Observe the living-row lifecycle: a merged PR + linked issue is a
  // shipped-candidate; when the linked issue later moves on to different
  // work, the store promotes it (retiring the PR icon) and grows the tally.
  useEffect(() => {
    observeWork(workspace.workspace_id, {
      issueNumber: workspace.linked_issue?.number ?? null,
      issueTitle: workspace.linked_issue?.title ?? null,
      prNumber: workspace.pr_number,
      prState: workspace.pr_state,
    });
  }, [
    observeWork,
    workspace.workspace_id,
    workspace.linked_issue,
    workspace.pr_number,
    workspace.pr_state,
  ]);

  // Opening the workspace "sees" it — collapsing a done row to a one-liner.
  //
  // markSeen normally fires on the isActive false→true edge. But a workspace
  // that *settles into review while it is already the active one* never
  // crosses that edge, so its 2-line Done card (and LIVE membership) would
  // linger until navigation or the ~1h fade. `settledAt` is in the deps so
  // the effect also re-runs when an active row's settle time advances, and we
  // re-stamp lastSeenAt then. Choice (a): re-mark seen instantly on
  // settle-while-active — the user already has the workspace open and is
  // looking at it, so treating the settle as "seen" is spec-compliant (the
  // done row collapses once seen). No grace delay: the collapse is the
  // perceivable state change for a workspace the user is actively viewing.
  useEffect(() => {
    if (isActive) markSeen(workspace.workspace_id);
  }, [isActive, settledAt, markSeen, workspace.workspace_id]);

  const isWorking = workspaceStatus === "working";
  const isPermission = workspaceStatus === "permission";
  const isReview = workspaceStatus === "review";

  // A done row expands only until it's opened (seen) or ~1h has elapsed.
  // `isReviewExpanded` is the shared predicate the "gather on top" grouping
  // reuses so the two never disagree about what still counts as live.
  const reviewFresh =
    settledAt != null && Date.now() - settledAt < SETTLED_FADE_MS;
  const reviewExpanded = isReview && isReviewExpanded(settledAt, lastSeenAt);

  const isCard = isWorking || isPermission || reviewExpanded;

  // Only tick the shared coarse clock while a row needs a live elapsed/fade.
  const needsClock = isWorking || isPermission || reviewFresh;
  const now = useCoarseClock(needsClock);

  const statusElapsed = formatElapsed(
    statusMark != null ? now - statusMark.at : 0,
  );

  // Fading green ✓ on a just-finished row — decays over ~1h, and only once
  // the work has settled (never while a fresh agent is mid-flight).
  const settledOpacity =
    settledAt != null ? 1 - (now - settledAt) / SETTLED_FADE_MS : 0;
  const showSettledCheck =
    !isWorking && !isPermission && settledOpacity > 0.05;

  // Where the linked-issue chip lives: on line 1 for working/done cards, and
  // in the git line for idle rows (its original home + hover-slide).
  const issueOnTitleLine = isWorking || reviewExpanded;

  // Living row (README "The living row"): while the row is a live card
  // (working / permission / unseen-review — the isWorkspaceLive set, which
  // `isCard` mirrors), the title IS the work — the linked issue title, with
  // the issue chip beside it — not the worktree name. The worktree/branch
  // name stays on the mono git line (working cards) and, so it is never
  // orphaned on the 2-line permission/review cards, in the row's title
  // tooltip. Idle rows keep the worktree name (today's behavior); a renamed
  // row falls back to the worktree name once its new work goes idle.
  const displayTitle =
    isCard && workspace.linked_issue
      ? workspace.linked_issue.title
      : workspace.title;
  const titleTooltip =
    isCard && workspace.linked_issue && workspace.git_branch
      ? workspace.git_branch
      : undefined;

  // Idle-row "n shipped" tally: merged PRs promoted to history for this
  // workspace. Only on settled one-liners — expanded cards let the issue chip
  // carry the signal (README rule 4).
  const shippedCount = shipped?.length ?? 0;
  const showTally = !isCard && shippedCount > 0;
  const showTitleMute =
    isCard && workspace.notifications_muted;

  // Git line: on working cards (line 3) and on idle rows (line 2), but only
  // when there is something worth surfacing beyond a bare branch. Permission
  // / done cards carry their own line 2 instead.
  const gitLineHasContent =
    !!workspace.git_branch ||
    hasAheadBehind ||
    hasDiff ||
    !!tunnelKind ||
    (!issueOnTitleLine &&
      (!!workspace.linked_issue || workspace.notifications_muted));
  const showGitLine = isWorking
    ? gitLineHasContent
    : !isCard &&
      (isDirty ||
        hasAheadBehind ||
        !!tunnelKind ||
        !!workspace.linked_issue ||
        workspace.notifications_muted);

  const prOpen = workspace.pr_state === "OPEN";

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            role="button"
            tabIndex={0}
            onClick={handleActivate}
            onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") handleActivate(); }}
            className={cn(
              "group/row mx-1.5 flex pl-[18px] pr-2 text-sm cursor-pointer relative transition-colors",
              // Cards (working / needs-you / done) gain a soft container and a
              // touch more vertical breathing room; idle rows keep today's
              // compact one-liner treatment.
              isCard
                ? "py-1.5 rounded-[10px] border"
                : "py-1 rounded-lg hover:bg-muted/40",
              isWorking && "border-border/60 bg-muted/30",
              isPermission && "border-status-attention/25 bg-status-attention/5",
              reviewExpanded && "border-status-open/20 bg-status-open/5",
              isActive && "bg-muted",
            )}
          >
            {/* Icon column — size-5 to subordinate to project avatar.
                When a worktree workspace has a PR, the icon turns into a
                PR-state-colored button that opens the PR URL on click. */}
            <div className="relative size-5 flex items-center justify-center shrink-0 mr-2">
              {workspaceStatus === "working" ? (
                <WorkingIndicator
                  variant={indicatorVariant}
                  color={indicatorColor}
                />
              ) : (
                <>
                  {showWorkspaceIconAsPr ? (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          type="button"
                          onClick={handlePrClick}
                          disabled={!workspace.pr_url}
                          aria-label={
                            workspace.pr_number
                              ? `Open PR #${workspace.pr_number} on GitHub — ${prHumanState ?? "Pull request"}`
                              : `Open pull request on GitHub — ${prHumanState ?? ""}`
                          }
                          className={cn(
                            "inline-flex items-center justify-center rounded transition-opacity",
                            workspace.pr_url ? "hover:opacity-70" : "cursor-not-allowed opacity-60",
                          )}
                        >
                          {icon}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                        {prHumanState ? `${prHumanState} PR` : "Pull request"}
                        {workspace.pr_number ? ` #${workspace.pr_number}` : ""}
                        {workspace.pr_url ? " — click to open" : ""}
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    icon
                  )}
                  {/* StatusIndicator is rendered as a sibling of the
                      icon/button and positioned absolutely relative to
                      the parent `relative size-5` div, so the agent-state
                      dots (working amber/pulsing, review green, permission
                      red/pulsing) still overlay the top-right corner of
                      the icon column regardless of whether the icon is
                      the plain branch icon or the new PR-state icon. */}
                  {workspaceStatus && (
                    <StatusIndicator
                      status={workspaceStatus}
                      className="absolute -top-0.5 -right-0.5"
                    />
                  )}
                </>
              )}
            </div>

            {/* Leading project chip — only in the LIVE section ("gather on
                top"), where a lifted row needs its project origin shown.
                Aligned to the icon row so it sits on line 1. */}
            {projectChip && (
              <div className="flex size-5 items-center shrink-0 -ml-1 mr-1.5">
                <ProjectAvatar
                  name={projectChip.name}
                  color={projectChip.color}
                  size="sm"
                  shape="square"
                  className="font-bold"
                />
              </div>
            )}

            <div className="flex-1 min-w-0">
              {/* Line 1: title + inline chips + trailing signals. Cards read
                  as work titles (slightly larger, semibold); idle rows keep
                  today's exact styling. */}
              <div className="flex items-center gap-1.5">
                <span
                  title={titleTooltip}
                  className={cn(
                  "truncate leading-tight",
                  isCard
                    ? "text-[13px] font-semibold text-foreground"
                    : cn(
                        "text-[13px]",
                        isActive
                          ? "text-foreground font-medium"
                          : "text-foreground/85",
                      ),
                )}>
                  {displayTitle}
                </span>

                {/* Linked-issue chip rides the title line for working / done
                    cards; idle rows keep it in the git line (below). */}
                {issueOnTitleLine && workspace.linked_issue && (
                  <IssueDetailPopover
                    workspaceId={workspace.workspace_id}
                    issue={workspace.linked_issue}
                  />
                )}

                {/* Muted bell — on the title line for cards (whose git line is
                    absent or issue-free); idle rows keep it in the git line. */}
                {showTitleMute && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <BellOff
                        className="h-3 w-3 shrink-0 text-muted-foreground/60"
                        aria-label="Notifications muted"
                      />
                    </TooltipTrigger>
                    <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                      Agent notifications muted
                    </TooltipContent>
                  </Tooltip>
                )}

                {(showSettledCheck ||
                  showTally ||
                  sidebarElapsedSec !== null ||
                  workspace.notification_count > 0) && (
                  <span className="ml-auto flex items-center gap-1.5 shrink-0">
                    {/* "n shipped" tally — merged PRs retired from this
                        workspace. Interactive (hover → popover of past work),
                        so it slides left to clear the hover-reveal archive
                        button, mirroring the idle row's issue cluster, rather
                        than fading like the non-interactive notification
                        badge. */}
                    {/* Outside the shared hover-card group (`@/lib/hover-card-group`):
                        it is nested inside the row's own workspace card's trigger, so
                        joining the group would have it supersede — and force-close —
                        the very card it sits on top of. It can therefore still stack
                        over the workspace card, which the group invariant otherwise
                        rules out. */}
                    {showTally && (
                      <HoverCard openDelay={120} closeDelay={80}>
                        <HoverCardTrigger asChild>
                          <button
                            type="button"
                            onClick={(e) => e.stopPropagation()}
                            aria-label={`${shippedCount} shipped`}
                            className="flex items-center gap-1 font-mono text-[11px] leading-none tabular-nums text-muted-foreground select-none transition-transform group-hover/row:-translate-x-8"
                          >
                            <span className="text-status-open">✓</span>
                            {shippedCount} shipped
                          </button>
                        </HoverCardTrigger>
                        <HoverCardContent
                          side="right"
                          align="start"
                          className="w-64 p-2"
                        >
                          <div className="mb-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground/70">
                            Shipped from this workspace
                          </div>
                          <ul className="space-y-1">
                            {shipped?.map((r) => (
                              <li
                                key={r.prNumber}
                                className="flex items-baseline gap-1.5 text-xs"
                              >
                                <span className="shrink-0 font-mono text-muted-foreground tabular-nums">
                                  #{r.issueNumber ?? r.prNumber}
                                </span>
                                {r.title && (
                                  <span className="truncate text-foreground/80">
                                    {r.title}
                                  </span>
                                )}
                              </li>
                            ))}
                          </ul>
                        </HoverCardContent>
                      </HoverCard>
                    )}
                    {/* Fading green ✓ on a just-finished / settled row — its
                        opacity decays over ~1h, then it disappears. */}
                    {showSettledCheck && (
                      <span
                        className="text-status-open text-[11px] leading-none tabular-nums select-none"
                        style={{ opacity: settledOpacity }}
                        aria-label="Recently finished"
                      >
                        ✓
                      </span>
                    )}
                    {sidebarElapsedSec !== null && (
                      <span
                        title="Push/pull in progress — large workspaces can take a while."
                        className="rounded-full bg-muted/60 px-1.5 py-0 text-[10px] font-medium tabular-nums leading-[14px] text-muted-foreground/85"
                      >
                        {sidebarElapsedSec}s
                      </span>
                    )}
                    {workspace.notification_count > 0 && (
                      <Badge
                        variant="outline"
                        className="text-[10px] tabular-nums text-warning bg-warning/15 border-transparent px-1.5 py-0 leading-[14px] h-[14px] transition-opacity group-hover/row:opacity-0"
                      >
                        {workspace.notification_count}
                      </Badge>
                    )}
                  </span>
                )}
              </div>

              {/* Line 2 (cards): activity / blocker / done. No backend
                  activity or permission text is reachable from the sidebar,
                  so this uses the derived-status fallback with a local
                  elapsed clock. */}
              {isWorking && (
                <div className="truncate text-[11px] text-muted-foreground leading-tight mt-0.5">
                  Working · {statusElapsed}
                </div>
              )}
              {isPermission && (
                <div className="truncate text-[11px] text-status-attention leading-tight mt-0.5">
                  {permissionBlockerText(workspace)} · {statusElapsed}
                </div>
              )}
              {reviewExpanded && (
                <div className="truncate text-[11px] leading-tight mt-0.5">
                  <span className="text-status-open font-medium">Done</span>
                  <span className="text-muted-foreground">
                    {prOpen
                      ? " · PR opened · review when ready"
                      : " · review when ready"}
                  </span>
                </div>
              )}

              {/* Git line — line 3 on working cards, line 2 on dirty / idle
                  rows. Mono branch + tunnel health + ahead/behind + diff,
                  plus the interactive linked-issue / mute cluster for idle
                  rows (with its hover-slide affordance). */}
              {showGitLine && (
                <div className={cn(
                  "flex items-center gap-1.5 font-mono leading-tight text-muted-foreground/60",
                  isWorking ? "text-[10px] mt-1" : "text-[11px] mt-0.5",
                )}>
                  {workspace.git_branch && (
                    <span className="truncate min-w-0">{workspace.git_branch}</span>
                  )}

                  {/* SSH tunnel health — only renders for a degraded remote
                      tunnel (reconnecting / circuit-open). A healthy or local
                      workspace shows nothing here. */}
                  {tunnelKind === "reconnecting" && (
                    <span className="shrink-0 rounded px-1 text-[10px] leading-[14px] text-warning bg-warning/15">
                      Reconnecting…
                    </span>
                  )}
                  {tunnelKind === "lost" && (
                    <span className="shrink-0 rounded px-1 text-[10px] leading-[14px] text-danger bg-danger/15">
                      Connection lost — re-push
                    </span>
                  )}

                  {hasAheadBehind && (
                    <span className={cn(
                      "flex items-center gap-1 shrink-0 tabular-nums",
                      // Fade out with the diff stats on hover so the issue
                      // chip never collides with the ahead/behind glyphs as
                      // it slides left to clear the archive button.
                      "transition-opacity group-hover/row:opacity-0",
                    )}>
                      {workspace.git_behind > 0 && (
                        <span className="text-warning/80">↓{workspace.git_behind}</span>
                      )}
                      {workspace.git_ahead > 0 && (
                        <span className="text-success/80">↑{workspace.git_ahead}</span>
                      )}
                    </span>
                  )}

                  {/* diff stats inline (no pill background) — fades on
                      hover so the hover-reveal archive button has its
                      slot. */}
                  {hasDiff && (
                    <span className={cn(
                      "flex items-center gap-1 shrink-0 tabular-nums ml-auto",
                      "transition-opacity group-hover/row:opacity-0",
                    )}>
                      {workspace.git_additions > 0 && (
                        <span className="text-success/80">+{workspace.git_additions}</span>
                      )}
                      {workspace.git_deletions > 0 && (
                        <span className="text-danger/80">−{workspace.git_deletions}</span>
                      )}
                    </span>
                  )}

                  {/* Indicator cluster — muted bell + linked issue. Kept on
                      idle rows; working cards surface the issue chip on the
                      title line instead. The PR signal lives entirely on the
                      leading icon column, so no PR number rides here. */}
                  {!issueOnTitleLine &&
                    (workspace.linked_issue || workspace.notifications_muted) && (
                    <div className={cn(
                      "flex items-center gap-1 shrink-0 rounded-md px-1",
                      !hasDiff && "ml-auto",
                      // The hover-reveal archive button is pinned at the
                      // right edge and overlays this slot. Unlike the diff
                      // stats / notification badge (which fade out on hover),
                      // the linked-issue badge is interactive, so instead of
                      // hiding it we slide the cluster left by the button's
                      // footprint (size-6 + right-2 ≈ 32px) so the issue stays
                      // visible and clickable while the button gets a clear
                      // slot. On hover it also gains an opaque chip (bg-muted
                      // + shadow, mirroring the archive button) so it reads as
                      // a distinct pill sitting cleanly over the branch name
                      // it now overlaps, instead of colliding glyph-on-glyph.
                      // Only the three properties the hover state actually
                      // changes are transitioned (duration/easing stay the
                      // defaults `transition-all` used). `transition-all`
                      // would also watch layout properties, so every row in
                      // the sidebar scroller pays for hover work it never
                      // does.
                      "transition-[transform,background-color,box-shadow] group-hover/row:-translate-x-8 group-hover/row:bg-muted group-hover/row:shadow-sm",
                    )}>
                      {workspace.notifications_muted && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <BellOff
                              className="h-3 w-3 text-muted-foreground/60"
                              aria-label="Notifications muted"
                            />
                          </TooltipTrigger>
                          <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                            Agent notifications muted
                          </TooltipContent>
                        </Tooltip>
                      )}
                      {workspace.linked_issue && (
                        <IssueDetailPopover
                          workspaceId={workspace.workspace_id}
                          issue={workspace.linked_issue}
                        />
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* Hover-reveal action button — pinned to a fixed slot so
                it lands at the same x-coordinate on every row. Rendered
                for EVERY row, including the primary/protected root.
                Local rows archive (non-destructive); attach-in-place /
                remote rows get the plain close instead, since the
                backend refuses to archive them. Shift-click on a
                deletable worktree opens the destructive delete dialog;
                on protected and attach/remote rows shift-click behaves
                like a plain click. */}
            <Button
              variant="ghost"
              size="icon-xs"
              className="absolute right-2 inset-y-0 my-auto opacity-0 group-hover/row:opacity-100 transition-opacity bg-muted text-muted-foreground shadow-sm hover:text-foreground dark:hover:bg-muted"
              onClick={(e) => {
                e.stopPropagation();
                if (e.shiftKey && canDelete) {
                  setShowDeleteDialog(true);
                  return;
                }
                void handleArchiveOrClose();
              }}
              aria-label={
                isAttachOrRemote ? "Close workspace" : "Archive workspace"
              }
            >
              {isAttachOrRemote ? (
                <X className="h-3.5 w-3.5" />
              ) : (
                <Archive className="h-3.5 w-3.5" />
              )}
            </Button>
          </div>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems
          workspace={workspace}
          onArchiveRequest={() => void handleArchiveOrClose()}
          onDeleteRequest={() => setShowDeleteDialog(true)}
          onRequestPushConfirm={(host) => setPendingPushHost(host)}
        />
      </ContextMenu>
      <DeleteWorktreeDialog
        workspace={workspace}
        open={showDeleteDialog}
        onOpenChange={setShowDeleteDialog}
      />
      <ConfirmPushDialog
        open={pendingPushHost !== null}
        workspaceTitle={workspace.title}
        host={pendingPushHost}
        onConfirm={() => {
          if (pendingPushHost) {
            void performPushToHost(pendingPushHost);
          }
        }}
        onOpenChange={(open) => {
          if (!open) setPendingPushHost(null);
        }}
      />
    </>
  );
}
