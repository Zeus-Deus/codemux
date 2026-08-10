import { useCallback, useEffect, useMemo, useState } from "react";
import { Terminal } from "lucide-react";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { ProviderLogo } from "@/components/chat/provider-logo";
import {
  isPrOnCurrentBranch,
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import {
  getWorkspaceProviders,
  STATUS_LABEL,
  STATUS_TEXT_CLASS,
} from "@/lib/pane-status";
import { shortenPath } from "@/lib/shorten-path";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import {
  formatElapsed,
  useSidebarDensityStore,
} from "@/stores/sidebar-density-store";
import {
  isHoverCardGroupActive,
  registerOpenHoverCard,
  useHoverCardGroupActive,
} from "@/lib/hover-card-group";
import { useProjectAppearance } from "./use-project-appearance";
import { cn } from "@/lib/utils";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import {
  providerForWorkspace,
  providerHostLabel,
  providerRef,
} from "@/lib/source-control";

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

/** How long the pointer must rest on a row before the FIRST card opens. Long
 *  enough that sweeping the mouse down the sidebar never flashes a card, short
 *  enough that a deliberate hover feels instant. Once a card is up the shared
 *  group phase takes over and the rest open with no delay at all — see
 *  `@/lib/hover-card-group` for why the second card should not re-pay this. */
const OPEN_DELAY_MS = 150;
/** Long enough for the pointer to cross the 10px offset gap into the card and
 *  select a path — but it never delays a sweep, because a card is superseded
 *  outright the moment another one opens (`registerOpenHoverCard`). */
const CLOSE_DELAY_MS = 100;

// `shortenPath`, `STATUS_LABEL`, and the status tone classes are shared with
// the command palette — see `@/lib/shorten-path` and `@/lib/pane-status`.

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
  const groupActive = useHoverCardGroupActive();
  // `instant` is captured when the card opens, not read live, and it governs
  // BOTH ends of the card's motion (see `data-instant` in
  // `@/components/ui/hover-card`). A card that DID wait its delay animates in
  // and fades out, even though the group is — by definition — active by the
  // time it closes. A card that skipped the delay gets neither: it appears at
  // once and hard-cuts on close, which is only ever mid-sweep, where a
  // fade-out would linger over the card that just replaced it.
  const [{ open, instant }, setCardState] = useState({
    open: false,
    instant: false,
  });

  const handleOpenChange = useCallback((next: boolean) => {
    // Read the store directly rather than closing over `groupActive`: this
    // fires from Radix's own timer, and the answer must be the phase as it
    // stands right now, before this card joins it below. Read OUTSIDE the
    // updater because updaters must be pure — React is free to re-run one
    // (StrictMode, a concurrent retry), and a second read could land after
    // this very card has joined the phase.
    const wasActive = isHoverCardGroupActive();
    setCardState((prev) => ({
      open: next,
      instant: next ? wasActive : prev.instant,
    }));
  }, []);

  useEffect(() => {
    if (!open) return;
    return registerOpenHoverCard(() =>
      setCardState((prev) => ({ ...prev, open: false })),
    );
  }, [open]);

  return (
    <HoverCard
      open={open}
      onOpenChange={handleOpenChange}
      openDelay={groupActive ? 0 : OPEN_DELAY_MS}
      closeDelay={CLOSE_DELAY_MS}
    >
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        side="right"
        align="start"
        sideOffset={10}
        entrance="subtle"
        data-instant={instant ? "" : undefined}
        // Pointer events stay enabled so the path and branch can be selected
        // and copied. Radix keeps the card open while the cursor crosses into
        // it, and side="right" floats it over the main pane — never between
        // the cursor and another sidebar row. The one exception is supersede:
        // reaching another row retires this card even mid-selection, because a
        // card held open over the row the user has moved on to is worse.
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
  // `pid: 0` is a sentinel, not a process id. The backend stamps it on
  // Docker-published container ports (one row per published host port) and on
  // static `.codemux/ports.json` entries, so counting raw pids collapsed a
  // whole compose stack into "1 running" and claimed a live process for a
  // workspace that had only *declared* its ports in config.
  //
  // Count evidence instead of rows: one per distinct real pid, plus one per
  // distinct Docker-published port — each of those is a container the daemon
  // reports as up. Static entries count for nothing; they say the workspace
  // uses a port, not that anything is listening on it, and the Ports row above
  // already lists them either way.
  const runningProcessCount = useMemo(() => {
    const pids = new Set<number>();
    const dockerPorts = new Set<number>();
    for (const p of ports) {
      if (p.pid !== 0) pids.add(p.pid);
      else if (p.source === "docker") dockerPorts.add(p.port);
    }
    return pids.size + dockerPorts.size;
  }, [ports]);
  const statusSince = useSidebarDensityStore(
    (s) => s.statusSince[workspace.workspace_id],
  );
  const settledAt = useSidebarDensityStore(
    (s) => s.settledAt[workspace.workspace_id],
  );

  const providers = getWorkspaceProviders(workspace.surfaces);
  const prState = normalizePrState(workspace.pr_state);
  // `scProvider` — the *hosting* product. Distinct from `providers`
  // above, which is the set of AI agent backends running in this
  // workspace's panes.
  const scProvider = providerForWorkspace(workspace);
  const prHost = hostOf(workspace.pr_url);
  const prHeadBranch = workspace.pr_head_branch ?? null;
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
              status ? STATUS_TEXT_CLASS[status] : "text-muted-foreground/70",
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
            label={scProvider.nounTitle}
            value={`${providerRef(scProvider, workspace.pr_number)} · ${prState}`}
            valueClassName={prStatusTextClass(workspace.pr_state) ?? undefined}
          />
        )}
        {/* Only when the change request is NOT the checked-out branch's. This is the
            details surface, so it can afford to answer the question the badge
            raises: the workspace has a PR, yet the Branch row above says
            something else. Naming the head branch says the PR came off a side
            branch — and explains why merging it will not settle this card. On
            the ordinary matching case the row would be pure repetition. */}
        {prState && !isPrOnCurrentBranch(prHeadBranch, workspace.git_branch) && (
          <DetailRow
            label={`${scProvider.shortNoun} branch`}
            value={prHeadBranch!}
            muted
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
        {/* Detected hosting product. Only shown once detection has
            actually classified this checkout: on a workspace with no
            `provider_kind` the presentation map falls back to GitHub for
            copy purposes, but stating "GitHub" here would be asserting a
            detection result that does not exist. */}
        {workspace.provider_kind && (
          <DetailRow
            label="Hosting"
            value={providerHostLabel(scProvider, prHost)}
            muted
          />
        )}
        {workspace.notifications_muted && (
          <DetailRow label="Notifications" value="muted" muted />
        )}
        {runningProcessCount > 0 && (
          <DetailRow
            label={runningProcessCount === 1 ? "Process" : "Processes"}
            value={
              // `role="img"` collapses the glyph and its count into one
              // labelled node, the same way the sidebar indicator does — an
              // `aria-label` on a bare span is ignored by screen readers.
              <span
                role="img"
                aria-label={`${runningProcessCount} running ${runningProcessCount === 1 ? "process" : "processes"}`}
                className="inline-flex items-center gap-1.5 text-status-open"
              >
                <Terminal
                  aria-hidden="true"
                  className="size-3 shrink-0 animate-pulse"
                  strokeWidth={1.7}
                />
                <span>{runningProcessCount} running</span>
              </span>
            }
          />
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

/** Hostname behind a change-request link, when there is one.
 *
 *  The snapshot carries no instance hostname of its own, but the PR/MR
 *  URL names it, and that is the detail worth surfacing: a self-hosted
 *  deployment reads "GitLab · gitlab.acme.com" rather than a bare
 *  product name that could be anything. */
function hostOf(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    return new URL(url).hostname || null;
  } catch {
    return null;
  }
}

function DetailRow({
  label,
  value,
  valueClassName,
  muted,
}: {
  label: string;
  value: React.ReactNode;
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
