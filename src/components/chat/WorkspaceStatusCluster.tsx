import { useEffect, useState } from "react";
import { AppWindow, ArrowDown, ChevronUp } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";
import { useActiveWorkspace, useHomeDir } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  PrStatusIcon,
  humanizePrState,
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import {
  BackgroundBrowserIndicator,
  useBackgroundBrowserSession,
} from "@/components/browser/background-browser-indicator";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";
import { showNoGitState, useInitializeGit } from "@/hooks/use-initialize-git";
import { getGithubPrByPath, gitPullChanges } from "@/tauri/commands";
import type { PullRequestInfo } from "@/tauri/types";
import {
  providerForWorkspace,
  providerRef,
  providerRefLabel,
} from "@/lib/source-control";

/**
 * Context Row status cluster (design: `.design-import/Context Row.dc.html`)
 * — the passive git/PR status from the retired full-width workspace
 * context bar, relocated to sit RIGHT of the composer's scope row. Renders:
 *
 *  - the GUI-mode background-browser indicator (the same sky-tinted
 *    "Browser" pill shown compactly in terminal headers — shared
 *    `BackgroundBrowserIndicator`) when the workspace has a live,
 *    pane-less agent browser session; click opens the peek overlay,
 *  - a behind-count chip (↓N, warning tone) when the branch trails
 *    its upstream,
 *  - a change-request chip (tone-tinted per state) that opens it on
 *    the hosting product,
 *  - a linked-issue chip (Issue #N) that opens the issue detail
 *    popover upward — the same `IssueDetailPopover` (chip variant) the
 *    old bar rendered, so a thread's linked issue stays visible on the
 *    Context Row,
 *  - a "workspace details" button that opens a compact popover with
 *    the full picture (branch, base, behind, ahead, uncommitted diff,
 *    PR, issue, device) plus quick view / sync actions.
 *
 * Self-contained: reads the active workspace directly. This only ever
 * renders from inside an `AgentChatPane`, and `PaneContainer` only
 * ever mounts panes for the currently active workspace — so
 * `useActiveWorkspace()` always resolves to the pane's own workspace.
 * (That mount context also makes the `enableAgentChat`
 * browser-indicator gate implicit here: a disabled flag renders a
 * placeholder instead of the pane.)
 *
 * Renders nothing when there is no active workspace, or there is
 * neither a git branch, a no-git affordance, a background browser
 * session, nor a linked issue (nothing passive to report). The git
 * chips + details popover additionally require the branch, so a
 * git-less workspace with a live background browser shows the Browser
 * pill alone, and one with only a linked issue shows the Issue chip
 * alone — matching the old bar's visibility set
 * (`hasGit || noGit || prState || linked_issue || browser`). Non-git
 * project folders (`showNoGitState`) get the compact "Initialize Git"
 * chip instead of the git cluster.
 */
export function WorkspaceStatusCluster() {
  const workspace = useActiveWorkspace();
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const backgroundBrowserSession = useBackgroundBrowserSession(
    workspace?.workspace_id,
  );
  const [open, setOpen] = useState(false);
  const [prInfo, setPrInfo] = useState<PullRequestInfo | null>(null);
  const [pulling, setPulling] = useState(false);
  // Explicit "Initialize Git" affordance for non-git project folders —
  // This is the chat surface's home for the opt-in nudge (never
  // auto-`git init`).
  const { initialize, initializing } = useInitializeGit(workspace ?? null);

  const cwd = workspace ? (workspace.worktree_path ?? workspace.cwd) : null;
  const prNumber = workspace?.pr_number ?? null;

  // Fetch-on-open, mirroring `IssueDetailPopover`. `WorkspaceSnapshot`
  // doesn't carry `base_branch` (see `PullRequestInfo.base_branch` in
  // tauri/types.ts) — it only exists on the full PR detail fetch. On
  // failure `prInfo` stays null and the Base row simply doesn't render.
  useEffect(() => {
    if (!open || !cwd || !prNumber) return;
    let cancelled = false;
    getGithubPrByPath(cwd, prNumber)
      .then((info) => {
        if (!cancelled) setPrInfo(info);
      })
      .catch(() => {
        if (!cancelled) setPrInfo(null);
      });
    return () => {
      cancelled = true;
    };
  }, [open, cwd, prNumber]);

  if (!workspace) return null;
  const gitBranch = workspace.git_branch;
  const showNoGit = !gitBranch && showNoGitState(workspace, homeDir);
  if (
    !gitBranch &&
    !showNoGit &&
    !backgroundBrowserSession &&
    !workspace.linked_issue
  )
    return null;

  const prState = normalizePrState(workspace.pr_state);
  const prHumanState = humanizePrState(workspace.pr_state);
  const provider = providerForWorkspace(workspace);
  const showBehind = workspace.git_behind > 0;
  const showAhead = workspace.git_ahead > 0;
  const showUncommitted =
    workspace.git_additions > 0 || workspace.git_deletions > 0;

  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const hostName = isRemote
    ? (hosts.find((h) => h.id === workspace.host_id)?.name ?? "Remote host")
    : null;
  const deviceLabel = isRemote ? (hostName ?? "Remote host") : "this device";

  const projectName = basename(workspace.project_root ?? workspace.cwd);

  /* Linked-issue chip — opens the issue detail popover upward. Mirrors
     the old bottom bar's usage (chip variant, `side="top"`). Rendered
     right after the PR chip (the old bar's ordering); unlike the git
     chips it does not require a branch, so a branch-less workspace with
     a linked issue still shows it (standalone render below). */
  const issueChip = workspace.linked_issue ? (
    <IssueDetailPopover
      providerKind={workspace.provider_kind}
      workspaceId={workspace.workspace_id}
      issue={workspace.linked_issue}
      variant="chip"
      side="top"
      align="end"
    />
  ) : null;

  const handlePrClick = () => {
    if (workspace.pr_url) openUrl(workspace.pr_url).catch(console.error);
  };

  const handleSync = async () => {
    if (pulling || !cwd) return;
    setPulling(true);
    try {
      await gitPullChanges(cwd);
    } catch (err) {
      toast.error(`Sync failed: ${err}`);
    } finally {
      setPulling(false);
    }
  };

  return (
    <div className="flex shrink-0 items-center gap-0.5">
      {/* GUI-mode background browser indicator — first in the cluster,
          mirroring its old bottom-bar position (right side, before the
          PR chip). Opens the peek overlay. */}
      {backgroundBrowserSession && (
        <BackgroundBrowserIndicator workspaceId={workspace.workspace_id} />
      )}

      {showNoGit && (
        <button
          type="button"
          onClick={initialize}
          disabled={initializing}
          aria-label="Initialize a git repository in this project folder"
          title="This project is not a git repository — worktrees, diffs, and checkpoints are unavailable until one is initialized"
          className="inline-flex h-[26px] shrink-0 items-center rounded-md border px-2 font-mono text-[11px] font-semibold text-muted-foreground transition-colors hover:bg-foreground/[0.06] hover:text-foreground disabled:cursor-not-allowed disabled:opacity-60"
        >
          {initializing ? "Initializing…" : "Initialize Git"}
        </button>
      )}

      {gitBranch && (
        <>
          {showBehind && (
            <span
              className="inline-flex h-[26px] items-center gap-1 rounded-md px-2 font-mono text-[11px] tabular-nums text-warning"
              title={`${workspace.git_behind} commit${workspace.git_behind === 1 ? "" : "s"} behind`}
            >
              <ArrowDown className="size-3" />
              {workspace.git_behind}
            </span>
          )}

          {prState && (
            <button
              type="button"
              onClick={handlePrClick}
              disabled={!workspace.pr_url}
              aria-label={
                workspace.pr_number
                  ? `Open ${providerRefLabel(provider, workspace.pr_number)} on ${provider.name} — ${prHumanState ?? provider.nounTitle}`
                  : `Open ${provider.noun} on ${provider.name} — ${prHumanState ?? ""}`
              }
              title={
                workspace.pr_number
                  ? `${provider.nounTitle} ${providerRef(provider, workspace.pr_number)} — ${prHumanState ?? ""}`
                  : undefined
              }
              className={cn(
                "inline-flex h-[26px] shrink-0 items-center gap-1 px-1.5 font-mono text-[11px] font-semibold transition-opacity hover:enabled:opacity-80",
                prStatusTextClass(prState),
                !workspace.pr_url && "cursor-not-allowed opacity-60",
              )}
            >
              <PrStatusIcon state={prState} size={3.5} />
              {workspace.pr_number
                ? providerRef(provider, workspace.pr_number)
                : provider.shortNoun}
            </button>
          )}

          {issueChip}

          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger asChild>
              <button
                type="button"
                aria-label="Workspace details"
                title="Workspace details"
                className={cn(
                  "inline-flex h-[26px] shrink-0 items-center gap-1 rounded-md px-2 text-[12px] font-semibold text-foreground/80 outline-none transition-colors hover:bg-foreground/[0.09]",
                  open && "bg-foreground/[0.09]",
                )}
              >
                <AppWindow className="size-3.5" />
                <ChevronUp className="size-2.5 opacity-50" />
              </button>
            </PopoverTrigger>
            <PopoverContent
              side="top"
              align="end"
              sideOffset={8}
              className="w-[290px] p-0"
            >
              <div className="border-b px-3.5 py-2.5">
                <div className="truncate text-[13px] font-bold text-foreground">
                  {workspace.title}
                </div>
                <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
                  {projectName} · {deviceLabel}
                </div>
              </div>
              <div className="flex flex-col gap-0.5 p-1.5">
                <DetailRow label="Branch" value={gitBranch} muted />
                {prInfo?.base_branch && (
                  <DetailRow label="Base" value={prInfo.base_branch} muted />
                )}
                {showBehind && (
                  <DetailRow
                    label="Behind base"
                    value={`↓${workspace.git_behind}`}
                    valueClassName="text-warning"
                  />
                )}
                {showAhead && (
                  <DetailRow
                    label="Ahead"
                    value={`↑${workspace.git_ahead}`}
                    valueClassName="text-success"
                  />
                )}
                {showUncommitted && (
                  <DetailRow
                    label="Uncommitted"
                    value={`+${workspace.git_additions} −${workspace.git_deletions}`}
                  />
                )}
                {prState && (
                  <DetailRow
                    label={provider.nounTitle}
                    value={`${providerRef(provider, workspace.pr_number)} · ${prHumanState}`}
                    valueClassName={prStatusTextClass(workspace.pr_state) ?? undefined}
                  />
                )}
                {workspace.linked_issue && (
                  <DetailRow
                    label="Issue"
                    value={`#${workspace.linked_issue.number} · ${workspace.linked_issue.state}`}
                    valueClassName={
                      workspace.linked_issue.state === "Open"
                        ? "text-success"
                        : "text-muted-foreground"
                    }
                  />
                )}
                <DetailRow label="Location" value={deviceLabel} muted />
              </div>
              {(workspace.pr_url || showBehind) && (
                <div className="flex gap-1.5 border-t p-1.5">
                  {workspace.pr_url && (
                    <button
                      type="button"
                      onClick={handlePrClick}
                      className="h-[30px] flex-1 rounded-md border bg-background text-[12px] font-semibold text-foreground transition-colors hover:bg-foreground/[0.06]"
                    >
                      View {provider.shortNoun}{" "}
                      {providerRef(provider, workspace.pr_number)}
                    </button>
                  )}
                  {showBehind && (
                    <button
                      type="button"
                      onClick={handleSync}
                      disabled={pulling}
                      className="h-[30px] flex-1 rounded-md border bg-background text-[12px] font-semibold text-foreground transition-colors hover:bg-foreground/[0.06] disabled:cursor-not-allowed disabled:opacity-60"
                    >
                      {pulling ? "Syncing…" : `Sync ↓${workspace.git_behind}`}
                    </button>
                  )}
                </div>
              )}
            </PopoverContent>
          </Popover>
        </>
      )}

      {/* Branch-less workspace with a linked issue: the git block above
          didn't render, so show the Issue chip standalone. */}
      {!gitBranch && issueChip}
    </div>
  );
}

function DetailRow({
  label,
  value,
  valueClassName,
  muted,
}: {
  label: string;
  value: string;
  /** Explicit tone class for the value (warning/success/PR-state).
   *  Falls back to `muted` when omitted. */
  valueClassName?: string;
  muted?: boolean;
}) {
  return (
    <div className="flex items-center gap-2 rounded-md px-2 py-1.5 transition-colors hover:bg-foreground/[0.05]">
      <span className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">
        {label}
      </span>
      <span
        className={cn(
          "shrink-0 font-mono text-[11px] tabular-nums",
          valueClassName ?? (muted ? "text-muted-foreground" : "text-foreground"),
        )}
      >
        {value}
      </span>
    </div>
  );
}
