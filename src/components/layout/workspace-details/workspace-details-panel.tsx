/**
 * Workspace details — the card behind the square-menu toggle in the title
 * bar's fixed top-right cluster (design reference: T3 Code's thread details
 * card and DP Code's Environment panel).
 *
 * It gathers what used to be a row of title-bar buttons (Run, Open in
 * editor) together with the workspace's version-control state and its live
 * agents, so the band itself stays quiet:
 *
 *   Where        device · worktree/local
 *   Open in …    default editor, chevron picks another
 *   Run          the project's run command, chevron configures it
 *   ──────────
 *   Branch       ahead / behind
 *   PR           the branch's change request, when there is one
 *   Git action   Commit / Push / Pull / Sync / Publish, chevron for the rest
 *   Changes      +N −N, opens the Changes pane
 *   ──────────
 *   Agents       running subagents, workflow run, task list
 *
 * Like T3's card it is pinned and on by default: it sits in the workspace
 * column's top-right corner while you work, so agents can be watched as
 * they come and go, and only the toggle (or its shortcut) hides it — which
 * then sticks across restarts (`ui-store`). When the column is too narrow
 * to hold it beside the chat, it turns into a light-dismiss popover in the
 * same corner, closed until asked for.
 */
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Bot,
  ChevronDown,
  Download,
  FileDiff,
  Folder,
  FolderGit2,
  GitBranch,
  GitCommitHorizontal,
  GitPullRequestCreateArrow,
  ListChecks,
  Monitor,
  SquareMenu,
  Upload,
  Workflow,
} from "lucide-react";

import { openDelegatedChat } from "@/components/chat/delegation-actions";
import { TickingText } from "@/components/chat/TickingText";
import {
  PrStatusIcon,
  humanizePrState,
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import { EditorIcon } from "@/components/icons/editor-icon";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverAnchor,
  PopoverContent,
} from "@/components/ui/popover";
import { Spinner } from "@/components/ui/spinner";
import { useActiveChatTasks } from "@/hooks/use-active-chat-tasks";
import { useEditorLauncher } from "@/hooks/use-editor-launcher";
import { showNoGitState, useInitializeGit } from "@/hooks/use-initialize-git";
import { childThreadIdOf } from "@/lib/agent-chat/delegation";
import {
  formatElapsed,
  isRunning,
  subagentElapsedMs,
  subagentPaneModel,
  subagentRowTitle,
  subagentWaves,
} from "@/lib/agent-chat/subagents";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { openExternalUrl } from "@/lib/open-url";
import { providerForWorkspace, providerRef } from "@/lib/source-control";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useActiveWorkspace, useHomeDir } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useLocalDeviceName } from "@/stores/local-device-store";
import { useUIStore, type RightPanelTab } from "@/stores/ui-store";
import { useWorkspaceWorkflow } from "@/components/workflow/use-workspace-workflow";
import {
  getGitBranchInfo,
  gitFetchChanges,
  gitPullChanges,
  gitPushChanges,
} from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { EditorMenuItems } from "../editor-menu-items";
import { PaneActionButton } from "../right-panel/pane-actions";
import { RunButton } from "../run-button";
import {
  DETAILS_CHEVRON,
  DETAILS_ICON,
  DETAILS_LABEL,
  DETAILS_MENU_ICON,
  DETAILS_MENU_ITEM,
  DETAILS_ROW,
  DETAILS_SPLIT_GROUP,
  DETAILS_SPLIT_PRIMARY,
  DETAILS_SPLIT_SECONDARY,
  DETAILS_SPLIT_SEPARATOR,
  DETAILS_STATIC_ROW,
  DETAILS_TRAILING,
} from "./styles";

const EMPTY_MESSAGES: ChatViewItem[] = [];

/** At most this many live subagents get a row of their own; the summary
 *  row below them carries the rest. */
const MAX_SUBAGENT_ROWS = 4;

// ── Placement ──
//
// Sizes are in rem so the card and its corner scale with the interface-size
// setting, like the rem-sized chrome around it.

/** Card width — T3 Code's thread details card uses the same 280px (17.5rem). */
const DETAILS_CARD_WIDTH_REM = 17.5;
/** Top edge: just under the 40px (2.5rem) title-bar band. */
const DETAILS_CARD_TOP_REM = 2.75;
/** Bottom air when the card is clipped to the window. */
const DETAILS_CARD_BOTTOM_GAP_REM = 0.75;
/** Air the card keeps from the centered chat column beside it. */
const DETAILS_CARD_CLEARANCE_REM = 1;
/** Shorter than this and the card is a scrolling sliver: use the popover. */
const DETAILS_CARD_MIN_HEIGHT_REM = 10;
/** The chat column's max width (`CHAT_COLUMN` in chat-column.ts). */
const CHAT_COLUMN_MAX_REM = 47.5;

/**
 * Pinned while the card fits in the workspace column's top-right corner
 * without covering anything; otherwise `null` — the card becomes a popover
 * in that same corner. This is T3 Code's rule (pin while a readable chat
 * lane fits beside the card), with two Codemux differences: the chat stays
 * centered rather than shifting over, and only a surface that is one chat
 * pane has that free margin at all — a terminal, an editor or a split runs
 * edge to edge, so a pinned card would sit on its content.
 */
export function resolveDetailsCardPlacement({
  columnWidth,
  edgeGap,
  viewportHeight,
  remPx,
  soleChat,
}: {
  columnWidth: number;
  /** The card's inset from the column's right edge, px. */
  edgeGap: number;
  viewportHeight: number;
  remPx: number;
  /** The active surface is a single agent-chat pane. */
  soleChat: boolean;
}): { top: number; width: number; maxHeight: number } | null {
  if (!soleChat) return null;
  const width = DETAILS_CARD_WIDTH_REM * remPx;
  const top = DETAILS_CARD_TOP_REM * remPx;
  const chatWidth = Math.min(CHAT_COLUMN_MAX_REM * remPx, columnWidth - 2 * remPx);
  const chatRight = (columnWidth + chatWidth) / 2;
  const cardLeft = columnWidth - edgeGap - width;
  if (cardLeft - chatRight < DETAILS_CARD_CLEARANCE_REM * remPx) return null;
  const maxHeight = viewportHeight - top - DETAILS_CARD_BOTTOM_GAP_REM * remPx;
  if (maxHeight < DETAILS_CARD_MIN_HEIGHT_REM * remPx) return null;
  return { top, width, maxHeight };
}

/** Whether a workspace's active surface is exactly one agent-chat pane. */
export function isSoleChatSurface(workspace: WorkspaceSnapshot | null): boolean {
  const surface = workspace?.surfaces.find(
    (candidate) => candidate.surface_id === workspace.active_surface_id,
  );
  return surface?.root.kind === "agent_chat";
}

function subscribeResize(onChange: () => void) {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

const viewportHeight = () => window.innerHeight;

const TOGGLE_SELECTOR = "[data-testid='workspace-details-toggle']";

const CARD_SCROLL = "thin-scrollbar overflow-y-auto border border-border bg-popover";

/** The pinned card is part of the canvas, not a menu over it: a quieter
 *  lift than the popover's menu plume (`cm-menu-surface`). */
const PINNED_SURFACE = cn(
  CARD_SCROLL,
  "rounded-xl shadow-[0_6px_24px_-10px_oklch(0_0_0/0.3)] dark:shadow-[0_8px_28px_-12px_oklch(0_0_0/0.6)]",
);

const POPOVER_SURFACE = cn(CARD_SCROLL, "cm-menu-surface");

/**
 * Renders the card for the active workspace: pinned in the column's corner
 * when there is room, a light-dismiss popover in the same corner when there
 * is not. Mounted by the title bar, which owns the geometry it needs.
 */
export function WorkspaceDetailsHost({
  workspaceId,
  columnWidth,
  rightInset,
  edgeGap,
  remPx,
}: {
  workspaceId: string;
  /** Width of the workspace column (content row minus an open panel). */
  columnWidth: number;
  /** Distance from the window's right edge to the column's right edge. */
  rightInset: number;
  /** The card's inset from the column's right edge — the title bar lines
   *  it up with the end of the control run above it. */
  edgeGap: number;
  /** The live root font size (`useRootFontPx`). */
  remPx: number;
}) {
  const height = useSyncExternalStore(subscribeResize, viewportHeight);
  const workspace = useActiveWorkspace();
  const soleChat = isSoleChatSurface(workspace);
  const placement = resolveDetailsCardPlacement({
    columnWidth,
    edgeGap,
    viewportHeight: height,
    remPx,
    soleChat,
  });
  const mode = placement ? "pinned" : "popover";
  const setMode = useUIStore((s) => s.setWorkspaceDetailsMode);
  const pinnedOpen = useUIStore((s) => s.workspaceDetailsOpen);
  const popoverOpen = useUIStore((s) => s.workspaceDetailsPopoverOpen);
  const setPopoverOpen = useUIStore((s) => s.setWorkspaceDetailsPopoverOpen);
  const setHosted = useUIStore((s) => s.setWorkspaceDetailsHosted);
  const contentRef = useRef<HTMLDivElement>(null);
  /** Set when the popover closes while focus is inside it (Escape, the
   *  shortcut, a row that navigates): focus goes back to the toggle rather
   *  than being dropped on <body>. A click elsewhere still moves focus to
   *  wherever that click lands, after this runs. */
  const restoreFocusRef = useRef(false);

  // The toggle and its shortcut act on whichever form is showing.
  useEffect(() => {
    setMode(mode);
  }, [mode, setMode]);

  // The shortcut handler gates on this, so it never flips the saved
  // preference for a card that is not mounted (Settings, the new-project
  // screen, a utility page, a draft, a fully expanded panel).
  useEffect(() => {
    setHosted(true);
    return () => setHosted(false);
  }, [setHosted]);

  // Runs synchronously inside the store update, before React removes the
  // popover, so focus is still where the user left it.
  useEffect(
    () =>
      useUIStore.subscribe((state, prev) => {
        if (prev.workspaceDetailsPopoverOpen && !state.workspaceDetailsPopoverOpen) {
          restoreFocusRef.current =
            contentRef.current?.contains(document.activeElement) ?? false;
        }
      }),
    [],
  );

  const right = rightInset + edgeGap;
  const width = DETAILS_CARD_WIDTH_REM * remPx;

  if (placement) {
    if (!pinnedOpen) return null;
    return (
      <aside
        data-testid="workspace-details-card"
        data-mode="pinned"
        aria-label="Workspace details"
        className={cn(PINNED_SURFACE, "pointer-events-auto absolute z-20")}
        style={{
          top: placement.top,
          right,
          width: placement.width,
          maxHeight: placement.maxHeight,
        }}
      >
        <WorkspaceDetailsCard key={workspaceId} workspaceId={workspaceId} />
      </aside>
    );
  }

  return (
    <Popover open={popoverOpen} onOpenChange={setPopoverOpen}>
      {/* A zero-height anchor along the band's bottom edge, so the popover
          opens in the very corner the pinned card would occupy. */}
      <PopoverAnchor asChild>
        <span
          aria-hidden
          className="pointer-events-none absolute top-10"
          style={{ right, width, height: 0 }}
        />
      </PopoverAnchor>
      <PopoverContent
        ref={contentRef}
        align="end"
        sideOffset={remPx / 4}
        collisionPadding={8}
        data-testid="workspace-details-card"
        data-mode="popover"
        aria-label="Workspace details"
        className={cn(
          POPOVER_SURFACE,
          "max-h-(--radix-popover-content-available-height) p-0",
        )}
        style={{ width }}
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          if (restoreFocusRef.current) {
            document.querySelector<HTMLElement>(TOGGLE_SELECTOR)?.focus();
          }
          restoreFocusRef.current = false;
        }}
        // The toggle closes it itself; letting the outside-press close it
        // first would make that click reopen it.
        onInteractOutside={(e) => {
          const target = e.target as Element | null;
          if (target?.closest?.(TOGGLE_SELECTOR)) e.preventDefault();
        }}
      >
        <WorkspaceDetailsCard
          key={workspaceId}
          workspaceId={workspaceId}
          onNavigate={() => setPopoverOpen(false)}
        />
      </PopoverContent>
    </Popover>
  );
}

// ── Toggle ──

/** The title-bar button in the fixed corner cluster. */
export function WorkspaceDetailsToggle({ disabled = false }: { disabled?: boolean }) {
  const visible = useUIStore((s) =>
    s.workspaceDetailsMode === "pinned"
      ? s.workspaceDetailsOpen
      : s.workspaceDetailsPopoverOpen,
  );
  const toggle = useUIStore((s) => s.toggleWorkspaceDetails);
  const agentsRunning = useAgentsRunning(visible && !disabled);

  return (
    <span className="relative flex">
      <PaneActionButton
        label="Workspace details"
        icon={SquareMenu}
        active={visible && !disabled}
        disabled={disabled}
        testId="workspace-details-toggle"
        size="titlebar"
        shortcut="toggleWorkspaceDetails"
        onClick={toggle}
      />
      {/* Hidden card, live agents: a quiet dot says there is something to
          watch behind the button. */}
      {agentsRunning && (
        <span
          aria-hidden
          data-testid="workspace-details-activity-dot"
          className="pointer-events-none absolute right-1 top-1 size-1.5 rounded-full bg-status-working"
        />
      )}
    </span>
  );
}

/** True while the closed card hides running subagents or a workflow run. */
function useAgentsRunning(cardOpen: boolean): boolean {
  const workspace = useActiveWorkspace();
  const { threadId } = useActiveChatTasks(workspace);
  const subagentsRunning = useAgentChatStore((s) => {
    if (cardOpen || !threadId) return false;
    const messages = s.threads[threadId]?.messages;
    return (
      messages?.some(
        (item) => item.kind === "subagent_run" && item.subagents.some(isRunning),
      ) ?? false
    );
  });
  const workflow = useWorkspaceWorkflow(workspace);
  return (
    !cardOpen &&
    (subagentsRunning || workflow.run?.status === "running")
  );
}

// ── Card ──

function WorkspaceDetailsCard({
  workspaceId,
  onNavigate,
}: {
  workspaceId: string;
  /** Called after a row takes the user somewhere else. The popover form
   *  closes on it; the pinned card stays, as it sits beside the panel. */
  onNavigate?: () => void;
}) {
  const workspace = useActiveWorkspace();
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  if (!workspace || workspace.workspace_id !== workspaceId) return null;

  const onClose = () => onNavigate?.();
  const openPane = (pane: RightPanelTab) => {
    setRightPanelTab(workspaceId, pane);
    onClose();
  };

  return (
    <div className="flex flex-col">
      <WorkspaceSection workspace={workspace} onClose={onClose} />
      <VersionControlSection workspace={workspace} openPane={openPane} onClose={onClose} />
      <AgentsSection workspace={workspace} openPane={openPane} onNavigate={onClose} />
    </div>
  );
}

function Section({
  children,
  title,
  separated = true,
  testId,
}: {
  children: React.ReactNode;
  title?: React.ReactNode;
  separated?: boolean;
  testId?: string;
}) {
  return (
    <section
      data-testid={testId}
      className={cn("flex flex-col gap-px p-1.5", separated && "border-t border-border/65")}
    >
      {title && (
        <h3 className="flex h-7 items-center px-2.5 text-label font-medium text-muted-foreground">
          {title}
        </h3>
      )}
      {children}
    </section>
  );
}

// ── Workspace: where it runs, open in editor, run ──

function WorkspaceSection({
  workspace,
  onClose,
}: {
  workspace: WorkspaceSnapshot;
  onClose: () => void;
}) {
  const hosts = useHosts();
  const localName = useLocalDeviceName();
  const isRemote = workspace.host_id !== null && workspace.host_id !== undefined;
  const deviceName = isRemote
    ? (hosts.find((h) => h.id === workspace.host_id)?.name ?? "Remote host")
    : (localName ?? "This device");
  const isWorktree = !!workspace.worktree_path;

  return (
    <Section separated={false} testId="workspace-details-workspace">
      <div className={DETAILS_STATIC_ROW} title={workspace.worktree_path ?? workspace.cwd}>
        {/* One icon, like every other row, so the labels share a column;
            the checkout kind rides on the right with its own glyph. */}
        <Monitor className={DETAILS_ICON} />
        <span className={DETAILS_LABEL}>{deviceName}</span>
        <span className={cn(DETAILS_TRAILING, "font-sans")}>
          {isWorktree ? (
            <FolderGit2 className="size-3.5" aria-hidden />
          ) : (
            <Folder className="size-3.5" aria-hidden />
          )}
          {isWorktree ? "Worktree" : "Local"}
        </span>
      </div>
      <OpenInEditorRow onOpened={onClose} />
      <RunButton workspaceId={workspace.workspace_id} variant="panel" />
    </Section>
  );
}

function OpenInEditorRow({ onOpened }: { onOpened: () => void }) {
  const { editors, defaultEditor, defaultEditorId, isLoading, open, unavailable } =
    useEditorLauncher();
  if (unavailable) return null;

  const openWith = (editorId: string) => {
    open(editorId);
    onOpened();
  };

  return (
    <DropdownMenu>
      <div role="group" className={DETAILS_SPLIT_GROUP}>
        <button
          type="button"
          disabled={isLoading || !defaultEditor}
          onClick={() => defaultEditor && openWith(defaultEditor.id)}
          className={DETAILS_SPLIT_PRIMARY}
        >
          {defaultEditor ? (
            <EditorIcon id={defaultEditor.id} className="size-4 shrink-0" />
          ) : (
            <Monitor className={DETAILS_ICON} />
          )}
          <span className={DETAILS_LABEL}>
            {defaultEditor ? `Open in ${defaultEditor.name}` : "Open in editor"}
          </span>
        </button>
        <span aria-hidden className={DETAILS_SPLIT_SEPARATOR} />
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            disabled={isLoading}
            aria-label="Choose editor"
            title="Choose editor"
            className={DETAILS_SPLIT_SECONDARY}
          >
            <ChevronDown className={DETAILS_CHEVRON} />
          </button>
        </DropdownMenuTrigger>
      </div>
      <DropdownMenuContent align="end" className="w-56">
        <EditorMenuItems
          editors={editors}
          defaultEditorId={defaultEditorId}
          onOpen={openWith}
          itemClassName={DETAILS_MENU_ITEM}
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ── Version control: branch, PR, git action, changes ──

type GitOp = "push" | "publish" | "pull" | "sync" | "fetch";

const GIT_OP_BUSY: Record<GitOp, string> = {
  push: "Pushing…",
  publish: "Publishing…",
  pull: "Pulling…",
  sync: "Syncing…",
  fetch: "Fetching…",
};

const GIT_OP_NAME: Record<GitOp, string> = {
  push: "Push",
  publish: "Publish",
  pull: "Pull",
  sync: "Sync",
  fetch: "Fetch",
};

const GIT_OP_DONE: Record<GitOp, string> = {
  push: "Pushed",
  publish: "Branch published",
  pull: "Pulled",
  sync: "Synced",
  fetch: "Fetched",
};

/** The one git step this workspace most likely wants next. */
export function primaryGitAction(input: {
  changedFiles: number;
  ahead: number;
  behind: number;
  /** `null` until the branch's upstream has been read. */
  hasUpstream: boolean | null;
}): { kind: "commit" } | { kind: GitOp; label: string } {
  const { changedFiles, ahead, behind, hasUpstream } = input;
  if (changedFiles > 0) return { kind: "commit" };
  if (ahead > 0 && behind > 0) return { kind: "sync", label: `Sync ↓${behind} ↑${ahead}` };
  if (ahead > 0) return { kind: "push", label: `Push ${ahead}` };
  if (behind > 0) return { kind: "pull", label: `Pull ${behind}` };
  if (hasUpstream === false) return { kind: "publish", label: "Publish branch" };
  return { kind: "fetch", label: "Fetch" };
}

function VersionControlSection({
  workspace,
  openPane,
  onClose,
}: {
  workspace: WorkspaceSnapshot;
  openPane: (pane: RightPanelTab) => void;
  onClose: () => void;
}) {
  const homeDir = useHomeDir();
  const { initialize, initializing } = useInitializeGit(workspace);
  const branch = workspace.git_branch;

  if (!branch) {
    if (!showNoGitState(workspace, homeDir)) return null;
    return (
      <Section testId="workspace-details-git">
        <button
          type="button"
          onClick={initialize}
          disabled={initializing}
          className={DETAILS_ROW}
          title="Worktrees, diffs, and checkpoints need a git repository"
        >
          <GitBranch className={DETAILS_ICON} />
          <span className={DETAILS_LABEL}>
            {initializing ? "Initializing…" : "Initialize Git"}
          </span>
        </button>
      </Section>
    );
  }

  return (
    <Section testId="workspace-details-git">
      <BranchRow workspace={workspace} branch={branch} />
      <PullRequestRow workspace={workspace} onOpened={onClose} />
      <GitActionRow workspace={workspace} openPane={openPane} />
      <button
        type="button"
        onClick={() => openPane("changes")}
        className={DETAILS_ROW}
      >
        <FileDiff className={DETAILS_ICON} />
        <span className={DETAILS_LABEL}>Changes</span>
        <DiffStat
          files={workspace.git_changed_files}
          additions={workspace.git_additions}
          deletions={workspace.git_deletions}
        />
      </button>
    </Section>
  );
}

function BranchRow({
  workspace,
  branch,
}: {
  workspace: WorkspaceSnapshot;
  branch: string;
}) {
  const copy = () => {
    navigator.clipboard
      .writeText(branch)
      .then(() => toast.success("Branch name copied"))
      .catch(() => toast.error("Couldn't copy the branch name"));
  };
  const { git_ahead: ahead, git_behind: behind } = workspace;
  return (
    <button
      type="button"
      onClick={copy}
      className={DETAILS_ROW}
      title={`${branch} — click to copy`}
    >
      <GitBranch className={DETAILS_ICON} />
      <span className={cn(DETAILS_LABEL, "font-mono text-label")}>{branch}</span>
      {(ahead > 0 || behind > 0) && (
        <span className={DETAILS_TRAILING}>
          {behind > 0 && (
            <span
              className="inline-flex items-center text-warning"
              title={`${behind} commit${behind === 1 ? "" : "s"} behind`}
            >
              <ArrowDown className="size-3" />
              {behind}
            </span>
          )}
          {ahead > 0 && (
            <span
              className="inline-flex items-center text-success"
              title={`${ahead} commit${ahead === 1 ? "" : "s"} ahead`}
            >
              <ArrowUp className="size-3" />
              {ahead}
            </span>
          )}
        </span>
      )}
    </button>
  );
}

function PullRequestRow({
  workspace,
  onOpened,
}: {
  workspace: WorkspaceSnapshot;
  onOpened: () => void;
}) {
  const prState = normalizePrState(workspace.pr_state);
  if (!prState) return null;
  const provider = providerForWorkspace(workspace);
  const ref = workspace.pr_number
    ? providerRef(provider, workspace.pr_number)
    : provider.shortNoun;
  return (
    <button
      type="button"
      disabled={!workspace.pr_url}
      onClick={(event) => {
        if (!workspace.pr_url) return;
        void openExternalUrl(workspace.pr_url, { event });
        onOpened();
      }}
      className={DETAILS_ROW}
      title={`Open on ${provider.name}`}
    >
      <PrStatusIcon state={prState} size={4} className={prStatusTextClass(prState) ?? undefined} />
      <span className={DETAILS_LABEL}>
        {provider.nounTitle} {ref}
      </span>
      <span className={cn(DETAILS_TRAILING, "font-sans", prStatusTextClass(prState))}>
        {humanizePrState(workspace.pr_state)}
      </span>
    </button>
  );
}

function GitActionRow({
  workspace,
  openPane,
}: {
  workspace: WorkspaceSnapshot;
  openPane: (pane: RightPanelTab) => void;
}) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  // Everything this row learns or starts belongs to one checkout + branch.
  // A result that lands after the branch changed (or a publish that
  // finishes after it) describes the old branch and must not leak into the
  // new one, so state is stored against this key and ignored once stale.
  const contextKey = `${cwd}\n${workspace.git_branch ?? ""}`;
  const [upstreamByKey, setUpstreamByKey] = useState<Record<string, boolean | null>>({});
  const setUpstream = (key: string, value: boolean | null) =>
    setUpstreamByKey((current) => ({ ...current, [key]: value }));
  const [busyState, setBusyState] = useState<{ key: string; op: GitOp } | null>(null);
  const hasUpstream = upstreamByKey[contextKey] ?? null;
  const busy = busyState?.key === contextKey ? busyState.op : null;
  const provider = providerForWorkspace(workspace);
  const hasPr = normalizePrState(workspace.pr_state) !== null;

  // Ahead/behind come from the workspace snapshot; whether the branch has
  // an upstream at all (and so needs Publish rather than Push) does not.
  useEffect(() => {
    let cancelled = false;
    getGitBranchInfo(cwd)
      .then((info) => {
        if (!cancelled) setUpstream(contextKey, info.has_upstream);
      })
      .catch(() => {
        if (!cancelled) setUpstream(contextKey, null);
      });
    return () => {
      cancelled = true;
    };
  }, [contextKey, cwd, workspace.git_ahead, workspace.git_behind]);

  const run = async (op: GitOp) => {
    if (busy) return;
    const key = contextKey;
    setBusyState({ key, op });
    try {
      if (op === "fetch") await gitFetchChanges(cwd);
      else if (op === "pull") await gitPullChanges(cwd);
      else if (op === "sync") {
        await gitPullChanges(cwd);
        await gitPushChanges(cwd, false);
      } else await gitPushChanges(cwd, op === "publish" || hasUpstream === false);
      // Recorded for the branch it published, never for whichever branch is
      // current by the time it finishes.
      if (op === "publish") setUpstream(key, true);
      toast.success(GIT_OP_DONE[op]);
    } catch (err) {
      toast.failure(`${GIT_OP_NAME[op]} failed`, err);
    } finally {
      setBusyState((current) => (current?.key === key ? null : current));
    }
  };

  const primary = primaryGitAction({
    changedFiles: workspace.git_changed_files,
    ahead: workspace.git_ahead,
    behind: workspace.git_behind,
    hasUpstream,
  });

  const primaryIcon =
    primary.kind === "commit" ? GitCommitHorizontal
    : primary.kind === "sync" ? ArrowUpDown
    : primary.kind === "push" || primary.kind === "publish" ? Upload
    : primary.kind === "pull" ? ArrowDown
    : Download;
  const PrimaryIcon = primaryIcon;

  return (
    <DropdownMenu>
      <div role="group" className={DETAILS_SPLIT_GROUP}>
        <button
          type="button"
          disabled={busy !== null}
          onClick={() =>
            primary.kind === "commit" ? openPane("changes") : void run(primary.kind)
          }
          className={DETAILS_SPLIT_PRIMARY}
          title={
            primary.kind === "commit"
              ? "Write a commit message in the Changes pane"
              : undefined
          }
        >
          {busy ? (
            <Spinner className="size-4 shrink-0 text-muted-foreground" />
          ) : (
            <PrimaryIcon className={DETAILS_ICON} />
          )}
          <span className={DETAILS_LABEL}>
            {busy
              ? GIT_OP_BUSY[busy]
              : primary.kind === "commit"
                ? "Commit"
                : primary.label}
          </span>
        </button>
        <span aria-hidden className={DETAILS_SPLIT_SEPARATOR} />
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="Git actions"
            title="Git actions"
            disabled={busy !== null}
            className={DETAILS_SPLIT_SECONDARY}
          >
            <ChevronDown className={DETAILS_CHEVRON} />
          </button>
        </DropdownMenuTrigger>
      </div>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuItem className={DETAILS_MENU_ITEM} onClick={() => openPane("changes")}>
          <GitCommitHorizontal className={DETAILS_MENU_ICON} />
          <span>Commit…</span>
        </DropdownMenuItem>
        <DropdownMenuItem
          className={DETAILS_MENU_ITEM}
          onClick={() => void run(hasUpstream === false ? "publish" : "push")}
        >
          <Upload className={DETAILS_MENU_ICON} />
          <span>{hasUpstream === false ? "Publish branch" : "Push"}</span>
        </DropdownMenuItem>
        <DropdownMenuItem
          className={DETAILS_MENU_ITEM}
          disabled={hasUpstream === false}
          onClick={() => void run("pull")}
        >
          <ArrowDown className={DETAILS_MENU_ICON} />
          <span>Pull</span>
        </DropdownMenuItem>
        <DropdownMenuItem className={DETAILS_MENU_ITEM} onClick={() => void run("fetch")}>
          <Download className={DETAILS_MENU_ICON} />
          <span>Fetch</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className={DETAILS_MENU_ITEM} onClick={() => openPane("review")}>
          <GitPullRequestCreateArrow className={DETAILS_MENU_ICON} />
          <span>
            {hasPr ? `Review ${provider.shortNoun}` : `Create ${provider.shortNoun}`}
          </span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function DiffStat({
  files,
  additions,
  deletions,
}: {
  files: number;
  additions: number;
  deletions: number;
}) {
  if (files === 0) return <span className={DETAILS_TRAILING}>clean</span>;
  // Untracked, binary and renamed-only files change no line counts.
  if (additions === 0 && deletions === 0) {
    return (
      <span className={DETAILS_TRAILING}>
        {files} {files === 1 ? "file" : "files"}
      </span>
    );
  }
  return (
    <span
      className={DETAILS_TRAILING}
      aria-label={`${additions} ${additions === 1 ? "line" : "lines"} added, ${deletions} removed`}
    >
      <span className="text-success/80">+{additions}</span>
      <span className="text-danger/80">−{deletions}</span>
    </span>
  );
}

// ── Agents: subagents, workflow, tasks ──

function AgentsSection({
  workspace,
  openPane,
  onNavigate,
}: {
  workspace: WorkspaceSnapshot;
  openPane: (pane: RightPanelTab) => void;
  onNavigate: () => void;
}) {
  const { threadId, tasks } = useActiveChatTasks(workspace);
  const messages = useAgentChatStore((s) =>
    threadId ? (s.threads[threadId]?.messages ?? EMPTY_MESSAGES) : EMPTY_MESSAGES,
  );
  const requestEnterSubagent = useUIStore((s) => s.requestEnterSubagent);
  const dismissed = useUIStore((s) => s.dismissedSubagentAttention);
  const workflow = useWorkspaceWorkflow(workspace);

  const model = useMemo(() => {
    if (!messages.some((item) => item.kind === "subagent_run")) return null;
    const waves = subagentWaves(messages);
    return { waves, ...subagentPaneModel(waves, new Set(dismissed)) };
  }, [messages, dismissed]);

  // Same gate as the right panel: a run still awaiting approval lives in
  // its approval card, not the Orchestration pane.
  const workflowRun =
    workflow.run && workflow.run.status !== "pending_approval" ? workflow.run : null;
  const taskList = tasks && tasks.tasks.length > 0 ? tasks.tasks : null;

  if (!model && !workflowRun && !taskList) return null;

  const running = model?.running ?? [];
  const shown = running.slice(0, MAX_SUBAGENT_ROWS);

  const openSubagent = (subagentId: string) => {
    // A delegated task is a real chat of its own: open its tab rather than
    // the read-only drill-in, which has nothing to show for it.
    const childThread = childThreadIdOf(subagentId);
    if (childThread) void openDelegatedChat(childThread);
    else if (threadId) requestEnterSubagent(threadId, subagentId);
    onNavigate();
  };

  return (
    <Section
      testId="workspace-details-agents"
      title={
        <>
          Agents
          {running.length > 0 && (
            <span className="ml-1 text-status-working">· {running.length} running</span>
          )}
        </>
      }
    >
      {shown.map((view) => (
        <button
          key={view.id}
          type="button"
          onClick={() => openSubagent(view.id)}
          className={DETAILS_ROW}
        >
          <span className="flex size-4 shrink-0 items-center justify-center">
            <span className="size-1.5 rounded-full bg-status-working motion-safe:animate-pulse" />
          </span>
          <span className={DETAILS_LABEL}>{subagentRowTitle(view)}</span>
          <TickingText
            className={DETAILS_TRAILING}
            compute={(now) => {
              const ms = subagentElapsedMs(view, now);
              return ms == null ? "" : formatElapsed(ms);
            }}
          />
        </button>
      ))}
      {model && (
        <button
          type="button"
          onClick={() => openPane("subagents")}
          className={DETAILS_ROW}
        >
          <Bot className={DETAILS_ICON} />
          <span className={DETAILS_LABEL}>
            {running.length > shown.length
              ? `${running.length - shown.length} more running`
              : "Subagents"}
          </span>
          {model.finishedCount > 0 && (
            <span className={DETAILS_TRAILING}>{model.finishedCount} done</span>
          )}
        </button>
      )}
      {workflowRun && (
        <button
          type="button"
          onClick={() => openPane("orchestration")}
          className={DETAILS_ROW}
        >
          <Workflow className={DETAILS_ICON} />
          <span className={DETAILS_LABEL}>{workflowRun.name ?? "Workflow"}</span>
          <span
            className={cn(
              DETAILS_TRAILING,
              "font-sans",
              workflowRun.status === "running" && "text-status-working",
              workflowRun.status === "failed" && "text-danger",
            )}
          >
            {workflowRun.status}
          </span>
        </button>
      )}
      {taskList && (
        <button
          type="button"
          onClick={() => openPane("tasks")}
          className={DETAILS_ROW}
        >
          <ListChecks className={DETAILS_ICON} />
          <span className={DETAILS_LABEL}>Tasks</span>
          <span className={DETAILS_TRAILING}>
            {taskList.filter((t) => t.status === "completed").length}/{taskList.length}
          </span>
        </button>
      )}
    </Section>
  );
}
