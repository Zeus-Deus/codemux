import { Cloud, Folder, GitBranch, Laptop } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { useActiveWorkspace } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useHosts } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { useAgentChatPaneActive } from "@/hooks/use-gui-chrome";
import {
  BackgroundBrowserIndicator,
  useBackgroundBrowserSession,
} from "@/components/browser/background-browser-indicator";
import {
  PR_CHIP_TONE,
  PrStatusIcon,
  humanizePrState,
  normalizePrState,
} from "@/components/github/pr-status-icon";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";

/**
 * Workspace context bar — the passive, read-only status strip under the
 * work surface. One home for the active workspace's full git detail
 * (branch · kind · ↑↓ · diff · files) plus clickable PR / issue chips
 * and the device it runs on.
 *
 * This is the "detail" half of the sidebar-density split: the sidebar
 * keeps the glanceable identity (name + branch + status) and this bar
 * holds the labeled numbers, in every sidebar mode (clean/branch/
 * detailed) — so nothing is lost when the sidebar is set to Clean.
 *
 * Hidden when there is nothing to report: no active workspace, a
 * brand-new (unscoped) chat draft, the onboarding wizard, a workspace
 * with no git context at all (e.g. a home-dir workspace), or — in GUI
 * chrome — an active Agent Chat pane, which now carries the same
 * detail inline in its own Context Row under the composer (see
 * `docs/features/agent-chat.md` "Context Row").
 */
export function WorkspaceContextBar() {
  // Hooks run unconditionally; all visibility gates come after.
  //
  // `useActiveWorkspace()` (full-object selector) is intentional here:
  // the bar renders most of the snapshot's git/PR/issue fields, and its
  // re-render is a few dozen cheap DOM nodes — nothing like the
  // markdown-reparse cascades the primitive selectors exist to avoid.
  const workspace = useActiveWorkspace();
  const lazyEnabled = useFeatureFlags((s) => s.enableLazyWorkspaceCreation);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const hasActiveDraft = useChatDraftStore((s) => s.activeDraftId !== null);
  const onboardingProjectDir = useUIStore((s) => s.onboardingProjectDir);
  const hosts = useHosts();
  // The Context Row (below the composer) now owns this workspace's
  // git/PR detail while an Agent Chat pane is the active surface —
  // showing both would duplicate the same numbers twice on screen.
  // A terminal (or other) pane active in GUI mode keeps this bar.
  const agentChatPaneActive = useAgentChatPaneActive();
  // GUI-mode background browser session for the active workspace —
  // shared lookup with the Context Row's status cluster (see the
  // hook's doc comment).
  const backgroundBrowserSession = useBackgroundBrowserSession(
    workspace?.workspace_id,
  );

  // Brand-new chat draft: the workspace scope isn't locked in yet, so
  // there is nothing to report (mirrors WorkspaceMain's draft branch).
  if (lazyEnabled && hasActiveDraft) return null;
  if (!workspace) return null;
  if (agentChatPaneActive) return null;

  // Onboarding wizard occupies the content area for this workspace.
  const isOnboarding =
    onboardingProjectDir !== null &&
    (workspace.project_root === onboardingProjectDir ||
      workspace.cwd === onboardingProjectDir);
  if (isOnboarding) return null;

  const prState = normalizePrState(workspace.pr_state);
  const hasGit = !!workspace.git_branch;

  const showBrowserIndicator =
    enableAgentChat &&
    workspace.workspace_type !== "open_flow" &&
    !!backgroundBrowserSession;

  // Nothing to report at all (e.g. a home-directory workspace).
  if (!hasGit && !prState && !workspace.linked_issue && !showBrowserIndicator) {
    return null;
  }

  const isWorktree = workspace.workspace_kind
    ? workspace.workspace_kind === "worktree"
    : !!workspace.worktree_path;
  const kindLabel = isWorktree ? "worktree" : "repo root";

  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const hostName = isRemote
    ? hosts.find((h) => h.id === workspace.host_id)?.name ?? "Remote host"
    : null;

  const prHumanState = humanizePrState(workspace.pr_state);
  const handlePrClick = () => {
    if (workspace.pr_url) {
      openUrl(workspace.pr_url).catch(console.error);
    }
  };

  const showBehind = workspace.git_behind > 0;
  const showAhead = workspace.git_ahead > 0;
  const showAdditions = workspace.git_additions > 0;
  const showDeletions = workspace.git_deletions > 0;
  const showChanged = workspace.git_changed_files > 0;

  return (
    <footer
      aria-label="Workspace context"
      className="flex h-[42px] shrink-0 items-center gap-3.5 border-t bg-card px-4"
    >
      {hasGit && (
        <>
          {/* Branch */}
          <div className="flex min-w-0 items-center gap-2">
            <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate font-mono text-xs font-medium text-foreground">
              {workspace.git_branch}
            </span>
          </div>

          <span className="h-4 w-px shrink-0 bg-border" aria-hidden="true" />

          {/* Workspace kind */}
          <span className="flex shrink-0 items-center gap-1.5 text-muted-foreground">
            <Folder className="h-3 w-3" />
            <span className="font-mono text-[11px]">{kindLabel}</span>
          </span>

          {/* Git counters */}
          {(showBehind ||
            showAhead ||
            showAdditions ||
            showDeletions ||
            showChanged) && (
            <span className="flex shrink-0 items-center gap-2 font-mono text-[11px] tabular-nums">
              {showBehind && (
                <span className="text-warning">↓{workspace.git_behind}</span>
              )}
              {showAhead && (
                <span className="text-success">↑{workspace.git_ahead}</span>
              )}
              {showAdditions && (
                <span className="text-success">+{workspace.git_additions}</span>
              )}
              {showDeletions && (
                <span className="text-danger">−{workspace.git_deletions}</span>
              )}
              {showChanged && (
                <span className="text-muted-foreground">
                  {workspace.git_changed_files}{" "}
                  {workspace.git_changed_files === 1 ? "file" : "files"}
                </span>
              )}
            </span>
          )}
        </>
      )}

      <div className="ml-auto flex shrink-0 items-center gap-2">
        {/* GUI-mode background browser indicator — opens the peek overlay */}
        {showBrowserIndicator && (
          <BackgroundBrowserIndicator workspaceId={workspace.workspace_id} />
        )}

        {/* PR chip — opens the PR on GitHub */}
        {prState && (
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
              "inline-flex h-[26px] items-center gap-1.5 rounded-md border px-2.5 text-[11px] font-semibold transition-colors",
              PR_CHIP_TONE[prState],
              !workspace.pr_url && "cursor-not-allowed opacity-60",
            )}
          >
            <PrStatusIcon
              state={workspace.pr_state}
              size={3}
              className="text-current"
            />
            PR{workspace.pr_number ? ` #${workspace.pr_number}` : ""}
            {prHumanState ? ` · ${prHumanState}` : ""}
          </button>
        )}

        {/* Linked-issue chip — opens the issue detail popover upward */}
        {workspace.linked_issue && (
          <IssueDetailPopover
            workspaceId={workspace.workspace_id}
            issue={workspace.linked_issue}
            variant="chip"
            side="top"
            align="end"
          />
        )}

        {/* Device */}
        <span className="flex items-center gap-1.5 pl-1 text-[11px] text-muted-foreground">
          {isRemote ? (
            <Cloud className="h-3 w-3" />
          ) : (
            <Laptop className="h-3 w-3" />
          )}
          {isRemote ? hostName : "This device"}
        </span>
      </div>
    </footer>
  );
}
