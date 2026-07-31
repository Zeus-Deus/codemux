import { useMemo } from "react";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { ProviderLogo } from "@/components/chat/provider-logo";
import {
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { getWorkspaceProviders } from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import {
  formatElapsed,
  useSidebarDensityStore,
} from "@/stores/sidebar-density-store";
import { useProjectAppearance } from "./use-project-appearance";
import { cn } from "@/lib/utils";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/** Project identity for the hovered workspace — same shape the inbox card
 *  and the rail already resolve from the grouping pipeline. */
export interface HoverCardRepo {
  name: string;
  path: string;
}

interface Props {
  workspace: WorkspaceSnapshot;
  repo: HoverCardRepo;
  /** Derived agent status. Every caller — rail, inbox card, settled row —
   *  computes it from the live pane statuses; null means genuinely idle. */
  status: ActivePaneStatus | null;
  children: React.ReactNode;
}

/** How long the pointer must rest on a row before the card opens. Long enough
 *  that sweeping the mouse down the sidebar never flashes a card, short enough
 *  that a deliberate hover feels instant. */
const OPEN_DELAY_MS = 350;
const CLOSE_DELAY_MS = 120;

/** Collapse `$HOME` to `~` so long worktree paths stay readable in the narrow
 *  card. Only a real home-directory boundary counts: with home `/home/u`, a
 *  sibling like `/home/u2/project` must stay absolute, so the character after
 *  the prefix has to be a path separator (or the path IS home). Accepts both
 *  `/` and `\` since remote workspaces can live on Windows hosts. Falls back
 *  to the raw path when the home dir isn't known yet. */
function shortenPath(path: string, homeDir: string | null): string {
  if (!homeDir) return path;
  // A reported home dir may carry a trailing separator ("/home/u/"); strip it
  // so the boundary check below sees the bare prefix.
  const home = homeDir.replace(/[/\\]+$/, "");
  if (home.length === 0 || !path.startsWith(home)) return path;
  const next = path[home.length];
  if (next === undefined) return "~"; // the path IS the home dir
  if (next !== "/" && next !== "\\") return path; // sibling prefix, not home
  return `~${path.slice(home.length)}`;
}

const STATUS_LABEL: Record<ActivePaneStatus, string> = {
  working: "Working",
  permission: "Needs you",
  review: "Done · review",
};

const STATUS_TONE: Record<ActivePaneStatus, string> = {
  working: "text-status-working",
  permission: "text-status-attention",
  review: "text-status-open",
};

/**
 * Hover details for a sidebar workspace. The sidebar row is deliberately terse
 * — a truncated title, a branch, and a couple of counters — so everything the
 * row had to drop lands here: the full title, the linked issue, the complete
 * git picture (including `behind` and changed-file counts the row never shows),
 * PR state, detected ports, which device it runs on, and the real path on disk.
 *
 * Wraps its children as the trigger. Mount it INSIDE the context-menu wrapper
 * (on a different element than `ContextMenuTrigger`) so the two `asChild`
 * triggers never compose onto the same node.
 */
export function WorkspaceHoverCard({
  workspace,
  repo,
  status,
  children,
}: Props) {
  return (
    <HoverCard openDelay={OPEN_DELAY_MS} closeDelay={CLOSE_DELAY_MS}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        side="right"
        align="start"
        sideOffset={10}
        // Pointer events stay enabled so the path and branch can be selected
        // and copied. Radix keeps the card open while the cursor crosses into
        // it (closeDelay), and side="right" floats it over the main pane —
        // never between the cursor and another sidebar row.
        className="w-[290px] p-0"
      >
        <WorkspaceHoverCardBody
          workspace={workspace}
          repo={repo}
          status={status}
        />
      </HoverCardContent>
    </HoverCard>
  );
}

/** The card's contents, split out so its stores are only subscribed while a
 *  card is actually open — Radix unmounts closed content, so a sidebar of 20
 *  workspaces pays for zero of these hooks at rest. Exported so tests can
 *  assert on the rows without driving Radix's hover timers. */
export function WorkspaceHoverCardBody({
  workspace,
  repo,
  status,
}: {
  workspace: WorkspaceSnapshot;
  repo: HoverCardRepo;
  status: ActivePaneStatus | null;
}) {
  const appearance = useProjectAppearance(repo.path);
  const homeDir = useHomeDir();
  const hosts = useHosts();
  // Select the raw slice, then narrow in a memo: a selector that returns a
  // fresh array every call breaks `useSyncExternalStore`'s snapshot equality
  // and spins React into an infinite re-render.
  const detectedPorts = useAppStore((s) => s.appState?.detected_ports);
  const ports = useMemo(
    () =>
      (detectedPorts ?? []).filter(
        (p) => p.workspace_id === workspace.workspace_id,
      ),
    [detectedPorts, workspace.workspace_id],
  );
  const statusSince = useSidebarDensityStore(
    (s) => s.statusSince[workspace.workspace_id],
  );
  const settledAt = useSidebarDensityStore(
    (s) => s.settledAt[workspace.workspace_id],
  );

  const providers = getWorkspaceProviders(workspace.surfaces);
  const prState = normalizePrState(workspace.pr_state);
  const issue = workspace.linked_issue;

  // Elapsed since the current state began. Stamped client-side (the backend
  // sends no status-changed-at), so it reads "just now" after an app restart.
  // Ticks on the shared coarse (~30s) clock so a pointer resting on the card
  // never shows a frozen "4m" — and since Radix unmounts closed content, the
  // interval only runs while a card is actually open.
  const stateSince =
    status !== null ? (statusSince?.at ?? null) : (settledAt ?? null);
  const now = useCoarseClock(stateSince != null);
  const elapsed = stateSince != null ? formatElapsed(now - stateSince) : null;

  const isGit = workspace.is_git !== false;
  const hasUncommitted =
    workspace.git_additions > 0 || workspace.git_deletions > 0;

  // `host_id` alone decides remote-ness. The hosts list loads asynchronously,
  // so a name may not be resolvable yet — fall back to a neutral label rather
  // than letting an unresolved lookup claim the workspace is local.
  const isRemote = workspace.host_id != null;
  const host = isRemote
    ? (hosts.find((h) => h.id === workspace.host_id) ?? null)
    : null;
  const location = isRemote
    ? `${host?.name ?? "Another device"}${workspace.attach_only ? " · in place" : ""}`
    : "This device";

  // The worktree checkout is the path the user actually works in; remote_cwd
  // is the real path on the host for an attach-in-place workspace.
  const path = workspace.worktree_path ?? workspace.remote_cwd ?? workspace.cwd;

  return (
    <>
      <div className="border-b px-3.5 py-2.5">
        <div className="flex items-center gap-1.5">
          <ProjectAvatar
            name={repo.name}
            color={appearance.customColor}
            imageUrl={appearance.imageUrl}
            cacheBust={appearance.imageVersion}
            size="sm"
            shape="square"
            className="font-bold"
          />
          <span className="min-w-0 flex-1 truncate text-[11px] font-semibold tracking-[0.01em] text-muted-foreground/80">
            {repo.name}
          </span>
          {providers.map((p) => (
            <ProviderLogo
              key={p}
              provider={p}
              className="h-[13px] w-[13px] shrink-0 opacity-80"
            />
          ))}
          <span
            className={cn(
              "shrink-0 text-[11px] font-semibold",
              status ? STATUS_TONE[status] : "text-muted-foreground/70",
            )}
          >
            {status ? STATUS_LABEL[status] : "Idle"}
            {elapsed && (
              <span className="ml-1 font-normal tabular-nums opacity-70">
                {elapsed}
              </span>
            )}
          </span>
        </div>
        {/* Full title — the row truncates it, so this is often the whole
            reason the user hovered. */}
        <div className="mt-1 text-[13px] font-bold leading-[1.35] text-foreground">
          {workspace.title}
        </div>
        {issue && (
          <div className="mt-0.5 text-[11px] leading-snug text-muted-foreground">
            <span className="font-mono">#{issue.number}</span> {issue.title}
          </div>
        )}
      </div>

      <div className="flex flex-col gap-0.5 p-1.5">
        {isGit && workspace.git_branch && (
          <DetailRow label="Branch" value={workspace.git_branch} muted />
        )}
        {isGit && hasUncommitted && (
          <DetailRow
            label="Uncommitted"
            value={`+${workspace.git_additions} −${workspace.git_deletions}`}
          />
        )}
        {isGit && workspace.git_changed_files > 0 && (
          <DetailRow
            label="Changed files"
            value={String(workspace.git_changed_files)}
            muted
          />
        )}
        {isGit && workspace.git_ahead > 0 && (
          <DetailRow
            label="Ahead"
            value={`↑${workspace.git_ahead}`}
            valueClassName="text-success"
          />
        )}
        {isGit && workspace.git_behind > 0 && (
          <DetailRow
            label="Behind"
            value={`↓${workspace.git_behind}`}
            valueClassName="text-warning"
          />
        )}
        {/* A clean tree is worth stating outright — the row shows nothing at
            all in that case, which is ambiguous with "stats hidden". */}
        {isGit &&
          !hasUncommitted &&
          workspace.git_changed_files === 0 &&
          workspace.git_branch && (
            <DetailRow label="Working tree" value="clean" muted />
          )}
        {prState && (
          <DetailRow
            label="Pull request"
            value={`#${workspace.pr_number ?? ""} · ${prState}`}
            valueClassName={prStatusTextClass(workspace.pr_state) ?? undefined}
          />
        )}
        {issue && (
          <DetailRow
            label="Issue"
            value={`#${issue.number} · ${issue.state}`}
            valueClassName={
              issue.state === "Open" ? "text-success" : "text-muted-foreground"
            }
          />
        )}
        {ports.length > 0 && (
          <DetailRow
            label={ports.length === 1 ? "Port" : "Ports"}
            value={ports
              .slice(0, 3)
              .map((p) => `:${p.port}`)
              .join(" ")
              .concat(ports.length > 3 ? ` +${ports.length - 3}` : "")}
            valueClassName="text-status-remote"
          />
        )}
        <DetailRow
          label="Location"
          value={location}
          valueClassName={isRemote ? "text-status-remote" : undefined}
          muted={!isRemote}
        />
        {workspace.notifications_muted && (
          <DetailRow label="Notifications" value="muted" muted />
        )}
      </div>

      {/* Real path on disk, last: the least-scannable line, and the one users
          most often want to copy or confirm. Wraps rather than truncates. */}
      <div className="border-t px-3.5 py-2">
        <div className="break-all font-mono text-[10px] leading-relaxed text-muted-foreground/70">
          {shortenPath(path, homeDir)}
        </div>
      </div>
    </>
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
    // The label is short and known; the value can be a long worktree branch.
    // So the label holds its width and the VALUE truncates — the reverse
    // would leave you reading "Bran…" next to the thing you came to see.
    <div className="flex items-center gap-3 rounded-md px-2 py-1.5">
      <span className="shrink-0 text-[12px] text-muted-foreground">
        {label}
      </span>
      <span
        className={cn(
          "min-w-0 flex-1 truncate text-right font-mono text-[11px] tabular-nums",
          valueClassName ?? (muted ? "text-muted-foreground" : "text-foreground"),
        )}
      >
        {value}
      </span>
    </div>
  );
}
