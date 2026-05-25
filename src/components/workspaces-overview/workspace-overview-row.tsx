import { startTransition, useCallback, useEffect, useState } from "react";

import {
  ArrowDownLeft,
  ArrowUpRight,
  CloudOff,
  ExternalLink,
  GitBranch,
  Laptop,
  Loader2,
  MoreHorizontal,
  Pencil,
  Trash2,
  Workflow,
} from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useAppStore } from "@/stores/app-store";
import { useHostsStore } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { StatusIndicator } from "@/components/ui/status-indicator";
import {
  activateWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  renameWorkspace,
  workspacePullBack,
  workspacePushToHost,
  type HostView,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

import type { DivergenceInfo, OverviewItem } from "./use-overview-items";

interface Props {
  item: OverviewItem;
  /** True when this row's local workspace is the active one in the
   *  main pane. Always false for remote (sibling-device) items. */
  isAttached: boolean;
  /** Called after a successful "Open" — the overview should close so
   *  the user lands on the workspace they picked. */
  onAfterOpen: () => void;
  /** Called when the user clicks "Pull to this device" on a
   *  sibling-device (remote-kind) row. The section hoists this and
   *  opens the shared `PullToDeviceDialog`. */
  onRequestPull?: (item: Extract<OverviewItem, { kind: "remote" }>) => void;
  /** Phase-4 divergence info — non-null when this row's git HEAD
   *  differs from another row sharing the same (project_remote,
   *  git_branch). Rendered as a warning chip in the title row. */
  divergence?: DivergenceInfo | null;
}

/**
 * One workspace card in the overview. Two shapes depending on the
 * item kind:
 *  - local: clickable card opens the workspace; `⋯` menu offers
 *    rename / push-to-host / pull-back / delete.
 *  - remote: clickable area is informational only (no local pane
 *    to open). `⋯` menu offers "Pull to this device" (TBD —
 *    queues a follow-up implementation; this PR only shows the
 *    sibling-device row + metadata). The "Pending sync" pill on
 *    local rows surfaces the row's `dirty` flag.
 */
export function WorkspaceOverviewRow({
  item,
  isAttached,
  onAfterOpen,
  onRequestPull,
  divergence,
}: Props) {
  if (item.kind === "remote") {
    return (
      <RemoteRow
        item={item}
        onRequestPull={onRequestPull ?? null}
        divergence={divergence ?? null}
      />
    );
  }
  return (
    <LocalRow
      item={item}
      isAttached={isAttached}
      onAfterOpen={onAfterOpen}
      divergence={divergence ?? null}
    />
  );
}

/**
 * Divergence chip — rendered in the row's title line when the
 * same (project_remote, git_branch) has different HEADs across
 * devices. Yellow, compact, with a tooltip pointing at the OTHER
 * location so the user knows where the other commits live.
 */
function DivergenceChip({ info }: { info: DivergenceInfo }) {
  return (
    <span
      title={`Diverged — different commits on ${info.otherLabel}. Push from one device to share, or pull to overwrite the other.`}
      className="shrink-0 inline-flex items-center gap-1 rounded-full border border-amber-500/30 bg-amber-500/10 px-1.5 py-0 text-[10px] font-medium leading-[14px] text-amber-300"
    >
      <svg
        viewBox="0 0 12 12"
        width="9"
        height="9"
        aria-hidden
        fill="currentColor"
      >
        <path d="M6 1.2L11.2 10.5H0.8L6 1.2zM5.3 4.8v2.4h1.4V4.8H5.3zM5.3 8v1.2h1.4V8H5.3z" />
      </svg>
      diverged
    </span>
  );
}

// ─── Local (has a WorkspaceSnapshot) ─────────────────────────────

function LocalRow({
  item,
  isAttached,
  onAfterOpen,
  divergence,
}: {
  item: Extract<OverviewItem, { kind: "local" }>;
  isAttached: boolean;
  onAfterOpen: () => void;
  divergence: DivergenceInfo | null;
}) {
  const workspace = item.workspace;
  const sync = item.sync;
  const project = item.projectName
    ? { name: item.projectName, path: item.projectPath ?? "" }
    : null;

  const hosts = useHostsStore((s) => s.hosts);
  const hostsLoaded = useHostsStore((s) => s.loaded);
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );
  const inFlight = useAppStore(
    (s) => s.workspacePushPullInFlight === workspace.workspace_id,
  );

  // Live agent status — same source the sidebar reads.
  const workspaceStatus: ActivePaneStatus | null = useAppStore((s) => {
    if (!s.appState) return null;
    return getWorkspaceStatus(workspace.surfaces, s.appState.pane_statuses);
  });
  const isWorking = workspaceStatus === "working";

  const [busy, setBusy] = useState(false);
  const isRemote = item.hostServerId !== null;
  const isWorktree = !!workspace.worktree_path;

  // Phase-4d elapsed-time indicator: when a push/pull takes longer
  // than ~2s, surface "12s elapsed" so the user knows it's working
  // (not stalled). Updates once per second via setInterval, only
  // while in-flight, so idle rows pay no re-render cost.
  const startedAt = useAppStore(
    (s) =>
      s.workspacePushPullInFlight === workspace.workspace_id
        ? s.workspacePushPullStartedAt
        : null,
  );
  const [elapsedSec, setElapsedSec] = useState<number | null>(null);
  useEffect(() => {
    if (startedAt === null) {
      setElapsedSec(null);
      return;
    }
    const tick = () => {
      const ms = Date.now() - startedAt;
      // Only surface once we've been waiting >2s, otherwise the
      // user is just seeing the spinner and the number would be a
      // visual jitter for fast operations.
      setElapsedSec(ms < 2_000 ? null : Math.floor(ms / 1_000));
    };
    tick();
    const id = window.setInterval(tick, 1_000);
    return () => window.clearInterval(id);
  }, [startedAt]);

  const handleOpen = useCallback(() => {
    if (inFlight) return;
    startTransition(() => {
      activateWorkspace(workspace.workspace_id)
        .then(() => {
          setShowWorkspacesOverview(false);
          onAfterOpen();
        })
        .catch((err) => {
          toast.error("Couldn't open workspace", {
            description: err instanceof Error ? err.message : String(err),
          });
        });
    });
  }, [inFlight, workspace.workspace_id, setShowWorkspacesOverview, onAfterOpen]);

  const handlePushToHost = useCallback(
    async (host: HostView) => {
      setPushPullInFlight(workspace.workspace_id);
      setBusy(true);
      try {
        const result = await workspacePushToHost(
          workspace.workspace_id,
          host.id,
        );
        if (result.ok) {
          toast.success(`Pushed to ${host.name}`, {
            description: result.message,
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
        setBusy(false);
      }
    },
    [workspace.workspace_id, setPushPullInFlight],
  );

  const handlePullBack = useCallback(async () => {
    setPushPullInFlight(workspace.workspace_id);
    setBusy(true);
    try {
      const result = await workspacePullBack(workspace.workspace_id);
      if (result.ok) {
        toast.success("Pulled back to this device", {
          description: result.message,
        });
      } else {
        toast.error("Pull back failed", {
          description: result.message,
        });
      }
    } catch (err) {
      toast.error("Pull back failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setPushPullInFlight(null);
      setBusy(false);
    }
  }, [workspace.workspace_id, setPushPullInFlight]);

  const handleRename = useCallback(() => {
    const next = window.prompt("Rename workspace", workspace.title);
    if (next && next !== workspace.title) {
      renameWorkspace(workspace.workspace_id, next).catch((err) =>
        toast.error("Rename failed", {
          description: err instanceof Error ? err.message : String(err),
        }),
      );
    }
  }, [workspace.workspace_id, workspace.title]);

  const handleDelete = useCallback(() => {
    const confirmed = window.confirm(
      isWorktree
        ? `Delete the worktree "${workspace.title}"? Files on disk will be removed.`
        : `Close "${workspace.title}"? Project files on disk are untouched.`,
    );
    if (!confirmed) return;
    const promise = isWorktree
      ? closeWorkspaceWithWorktree(workspace.workspace_id, true, true, false)
      : closeWorkspace(workspace.workspace_id, false);
    promise.catch((err) =>
      toast.error("Delete failed", {
        description: err instanceof Error ? err.message : String(err),
      }),
    );
  }, [workspace.workspace_id, workspace.title, isWorktree]);

  const handleCopyBranch = useCallback(() => {
    if (workspace.git_branch) {
      navigator.clipboard
        .writeText(workspace.git_branch)
        .then(() =>
          toast.success("Copied branch name", {
            description: workspace.git_branch ?? "",
          }),
        )
        .catch(console.error);
    }
  }, [workspace.git_branch]);

  const showingInFlight = inFlight || busy;
  const hasDiff =
    workspace.git_additions > 0 || workspace.git_deletions > 0;
  const hasAheadBehind = workspace.git_ahead > 0 || workspace.git_behind > 0;
  const hasDirty = workspace.git_changed_files > 0;

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          handleOpen();
        }
      }}
      className={cn(
        "group/row relative flex h-full cursor-pointer flex-col gap-2 rounded-lg border bg-card/40 px-3.5 py-3 transition-colors",
        "hover:border-border hover:bg-muted/40",
        isAttached
          ? "border-emerald-500/40 ring-1 ring-emerald-500/20"
          : "border-border/50",
      )}
    >
      <div className="flex items-start gap-2">
        <StatusDot
          attached={isAttached}
          remote={isRemote}
          inFlight={showingInFlight}
          openFlow={workspace.workspace_type === "open_flow"}
          workspaceStatus={workspaceStatus}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h4
              className={cn(
                "min-w-0 flex-1 truncate text-[13px] font-medium leading-tight",
                isAttached ? "text-foreground" : "text-foreground/90",
              )}
              title={workspace.title}
            >
              {workspace.title}
            </h4>
            {divergence && <DivergenceChip info={divergence} />}
            {sync?.dirty && (
              <span
                title="Pending sync to your account"
                className="shrink-0 rounded-full bg-warning/15 px-1.5 py-0 text-[10px] font-medium uppercase tracking-wider leading-[14px] text-warning"
              >
                sync
              </span>
            )}
            {workspace.notification_count > 0 && (
              <span
                title={`${workspace.notification_count} unread notification${workspace.notification_count === 1 ? "" : "s"}`}
                className="shrink-0 rounded-full bg-warning/15 px-1.5 py-0 text-[10px] font-medium tabular-nums leading-[14px] text-warning"
              >
                {workspace.notification_count}
              </span>
            )}
          </div>
          <p className="mt-0.5 truncate text-[11px] text-muted-foreground/65">
            {project ? project.name : "—"}
            {isWorking && (
              <span className="ml-1.5 text-amber-400/90">
                · agent working
              </span>
            )}
            {!isWorking && workspaceStatus === "permission" && (
              <span className="ml-1.5 text-red-400/90">· needs input</span>
            )}
            {!isWorking && workspaceStatus === "review" && (
              <span className="ml-1.5 text-emerald-400/90">
                · ready to review
              </span>
            )}
            {!workspaceStatus && isAttached && (
              <span className="ml-1.5 text-emerald-400/80">· open now</span>
            )}
            {!workspaceStatus && isRemote && !isAttached && (
              <span className="ml-1.5 text-sky-300/70">· remote</span>
            )}
          </p>
        </div>

        <div
          className={cn(
            "opacity-0 transition-opacity",
            "group-hover/row:opacity-100 group-focus-within/row:opacity-100",
            showingInFlight && "opacity-100",
          )}
        >
          {showingInFlight ? (
            <span
              aria-label={
                elapsedSec !== null
                  ? `Working — ${elapsedSec}s elapsed`
                  : "Working"
              }
              className="flex items-center gap-1.5 text-muted-foreground"
            >
              <Loader2 className="size-3.5 animate-spin" />
              {elapsedSec !== null && (
                <span
                  title="Push and pull use rsync over SSH — large workspaces can take a while. Stay on this page; you'll see a toast when it's done."
                  className="rounded-full bg-muted/60 px-1.5 py-0 text-[10px] font-medium tabular-nums leading-[14px] text-muted-foreground/85"
                >
                  {elapsedSec}s
                </span>
              )}
            </span>
          ) : (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Workspace actions"
                  onClick={(e) => e.stopPropagation()}
                  className="size-7 text-muted-foreground hover:text-foreground"
                >
                  <MoreHorizontal className="size-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="end"
                onClick={(e) => e.stopPropagation()}
                className="min-w-[200px]"
              >
                <DropdownMenuItem onClick={handleOpen}>
                  <ExternalLink className="mr-2 size-3.5" />
                  Open workspace
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={handleCopyBranch}
                  disabled={!workspace.git_branch}
                >
                  <GitBranch className="mr-2 size-3.5" />
                  Copy branch name
                </DropdownMenuItem>
                <DropdownMenuItem onClick={handleRename}>
                  <Pencil className="mr-2 size-3.5" />
                  Rename…
                </DropdownMenuItem>

                <DropdownMenuSeparator />

                {isRemote ? (
                  <DropdownMenuItem onClick={() => void handlePullBack()}>
                    <ArrowDownLeft className="mr-2 size-3.5" />
                    Pull back to this device
                  </DropdownMenuItem>
                ) : !hostsLoaded ? (
                  <DropdownMenuItem disabled>
                    <Loader2 className="mr-2 size-3.5 animate-spin" />
                    Loading hosts…
                  </DropdownMenuItem>
                ) : hosts.length === 0 ? (
                  <DropdownMenuItem
                    disabled
                    title="Add a device in Settings → Devices to push workspaces"
                  >
                    <ArrowUpRight className="mr-2 size-3.5" />
                    Push to device…
                  </DropdownMenuItem>
                ) : (
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <ArrowUpRight className="mr-2 size-3.5" />
                      Push to device…
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="min-w-[200px]">
                      <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground/60">
                        Send to
                      </DropdownMenuLabel>
                      {hosts.map((host) => (
                        <DropdownMenuItem
                          key={host.id}
                          onClick={() => void handlePushToHost(host)}
                        >
                          <span className="truncate">{host.name}</span>
                          <span className="ml-auto pl-2 font-mono text-[10px] text-muted-foreground/55">
                            {host.ssh_target}
                          </span>
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                )}

                <DropdownMenuSeparator />

                <DropdownMenuItem
                  onClick={handleDelete}
                  className="text-destructive focus:bg-destructive/10 focus:text-destructive"
                >
                  <Trash2 className="mr-2 size-3.5" />
                  {isWorktree ? "Delete worktree…" : "Close workspace…"}
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>

      {(workspace.git_branch || hasDiff || hasAheadBehind || hasDirty) && (
        <div className="flex items-center gap-2 pl-5 font-mono text-[11px] text-muted-foreground/60">
          {workspace.git_branch && (
            <span className="flex items-center gap-1 min-w-0">
              {workspace.workspace_type === "open_flow" ? (
                <Workflow className="size-3 shrink-0" aria-hidden />
              ) : isWorktree ? (
                <GitBranch className="size-3 shrink-0" aria-hidden />
              ) : (
                <Laptop className="size-3 shrink-0" aria-hidden />
              )}
              <span className="truncate">{workspace.git_branch}</span>
            </span>
          )}

          {hasAheadBehind && (
            <span className="flex shrink-0 items-center gap-1 tabular-nums">
              {workspace.git_behind > 0 && (
                <span className="text-warning/80">
                  ↓{workspace.git_behind}
                </span>
              )}
              {workspace.git_ahead > 0 && (
                <span className="text-success/80">
                  ↑{workspace.git_ahead}
                </span>
              )}
            </span>
          )}

          {hasDirty && !hasDiff && (
            <span
              className="shrink-0 text-warning/70"
              title={`${workspace.git_changed_files} changed files`}
            >
              {workspace.git_changed_files} changed
            </span>
          )}

          {hasDiff && (
            <span className="ml-auto flex shrink-0 items-center gap-1 tabular-nums">
              {workspace.git_additions > 0 && (
                <span className="text-success/80">
                  +{workspace.git_additions}
                </span>
              )}
              {workspace.git_deletions > 0 && (
                <span className="text-danger/80">
                  −{workspace.git_deletions}
                </span>
              )}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Remote (sibling-device, no WorkspaceSnapshot) ───────────────

function RemoteRow({
  item,
  onRequestPull,
  divergence,
}: {
  item: Extract<OverviewItem, { kind: "remote" }>;
  onRequestPull: ((item: Extract<OverviewItem, { kind: "remote" }>) => void) | null;
  divergence: DivergenceInfo | null;
}) {
  const row = item.sync;
  const branch = row.git_branch;
  const canRequestPull = onRequestPull !== null;

  return (
    <div
      role="group"
      aria-label={`${row.title}, lives on another device`}
      className={cn(
        "group/row relative flex h-full flex-col gap-2 rounded-lg border border-dashed border-sky-500/30 bg-sky-500/5 px-3.5 py-3",
        "hover:border-sky-500/50 hover:bg-sky-500/8 transition-colors",
      )}
    >
      <div className="flex items-start gap-2">
        <span
          aria-label="Lives on another device"
          title="This workspace lives on another device of your account."
          className="mt-[5px] flex size-2 shrink-0 rounded-full bg-sky-400/70"
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h4
              className="min-w-0 flex-1 truncate text-[13px] font-medium leading-tight text-foreground/90"
              title={row.title}
            >
              {row.title}
            </h4>
            {divergence && <DivergenceChip info={divergence} />}
            <span
              title="This workspace lives on another device of your account. Pull it down to interact with it here."
              className="shrink-0 rounded-full border border-sky-400/30 bg-sky-500/10 px-1.5 py-0 text-[10px] font-medium uppercase tracking-wider leading-[14px] text-sky-300"
            >
              other device
            </span>
          </div>
          <p className="mt-0.5 truncate text-[11px] text-muted-foreground/65">
            {item.projectName ?? "—"}
            <span className="ml-1.5 text-sky-300/70">
              · not on this device
            </span>
          </p>
        </div>

        <div
          className={cn(
            "opacity-0 transition-opacity",
            "group-hover/row:opacity-100 group-focus-within/row:opacity-100",
          )}
        >
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                aria-label="Workspace actions"
                onClick={(e) => e.stopPropagation()}
                className="size-7 text-muted-foreground hover:text-foreground"
              >
                <MoreHorizontal className="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              onClick={(e) => e.stopPropagation()}
              className="min-w-[220px]"
            >
              <DropdownMenuLabel className="text-[10px] uppercase tracking-wider text-muted-foreground/60">
                Lives on another device
              </DropdownMenuLabel>
              {canRequestPull ? (
                <DropdownMenuItem
                  onClick={() => onRequestPull?.(item)}
                >
                  <ArrowDownLeft className="mr-2 size-3.5" />
                  Pull to this device…
                </DropdownMenuItem>
              ) : (
                <DropdownMenuItem
                  disabled
                  title="Pulling sibling-device workspaces is unavailable here."
                >
                  <CloudOff className="mr-2 size-3.5" />
                  Pull to this device (unavailable)
                </DropdownMenuItem>
              )}
              <DropdownMenuSeparator />
              <DropdownMenuItem
                disabled={!branch}
                onClick={() => {
                  if (branch) {
                    navigator.clipboard.writeText(branch).catch(console.error);
                    toast.success("Copied branch name", {
                      description: branch,
                    });
                  }
                }}
              >
                <GitBranch className="mr-2 size-3.5" />
                Copy branch name
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>

      {branch && (
        <div className="flex items-center gap-1.5 pl-5 font-mono text-[11px] text-muted-foreground/60">
          <GitBranch className="size-3 shrink-0" aria-hidden />
          <span className="truncate">{branch}</span>
        </div>
      )}
    </div>
  );
}

// ─── Shared status dot ──────────────────────────────────────────

function StatusDot({
  attached,
  remote,
  inFlight,
  openFlow,
  workspaceStatus,
}: {
  attached: boolean;
  remote: boolean;
  inFlight: boolean;
  openFlow: boolean;
  workspaceStatus: ActivePaneStatus | null;
}) {
  if (workspaceStatus) {
    return (
      <span className="mt-[5px] flex shrink-0">
        <StatusIndicator status={workspaceStatus} />
      </span>
    );
  }

  const label = inFlight
    ? "Syncing"
    : attached
      ? "Currently open in this app"
      : remote
        ? "On a remote host"
        : openFlow
          ? "OpenFlow workspace"
          : "Local";

  return (
    <span
      aria-label={label}
      title={label}
      className={cn(
        "mt-[5px] flex size-2 shrink-0 items-center justify-center rounded-full",
        inFlight
          ? "bg-amber-400/80"
          : attached
            ? "bg-emerald-400"
            : remote
              ? "bg-sky-400/80"
              : openFlow
                ? "bg-violet-400/70"
                : "bg-muted-foreground/40",
      )}
    />
  );
}

// Silence unused-import warning when the type guard's narrowed shape
// makes the `WorkspaceSnapshot` import look unused in some build paths.
export type _OverviewWorkspace = WorkspaceSnapshot;
