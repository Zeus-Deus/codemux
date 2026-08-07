/**
 * The right panel — a **pane deck**.
 *
 * It used to be four hard-coded tabs (Files / Changes / Review / Tasks)
 * splitting a fixed 45px strip four ways, each pane growing its own
 * header underneath. It is now a deck with two fixed rows:
 *
 *   1. one 36px row of panel chrome — closable icon tabs and a `+` menu on
 *      the left, the active pane's own actions and the panel controls on
 *      the right (see `right-panel/pane-tab-strip.tsx`),
 *   2. a 26px status foot that follows the active pane.
 *
 * There was briefly a third band between them, a breadcrumb row reading
 * "<workspace> › <pane>". It said nothing the sidebar, the composer and
 * the tab label weren't already saying, and it pushed the first line of
 * real content down behind three stacked bars. Its buttons moved into the
 * tab row's right-hand slot; the "what am I looking at" text it carried
 * (the browser's URL, the diff's file) moved to the status foot.
 *
 * Panes are declared in `right-panel/pane-registry.ts`, opened/closed
 * like editor tabs, and persisted per workspace in `ui-store`
 * (`rightPanelPanes` = order, `rightPanelTabs` = active,
 * `rightPanelDismissedPanes` = "don't auto-reopen this one"). Nothing
 * that was reachable before is unreachable now: the four original tabs
 * are default panes, Review/Orchestration/Subagents are panes too, and
 * anything closed comes back from the `+` menu.
 *
 * Live activity in the strip is a mono badge, not a blinking dot — the
 * app's one working affordance is the orb (`src/components/ui/agent-orb`).
 */
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlignJustify,
  Check,
  Code2,
  Columns2,
  Copy,
  ExternalLink,
  Eye,
  EyeOff,
  FileText,
  ListFilter,
  Loader2,
  PanelRight as PanelRightIcon,
  RefreshCw,
  Terminal,
  WrapText,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";

import { TasksPanel, TasksPaneActions } from "@/components/chat/TasksPanel";
import { DiffPane } from "@/components/diff/DiffPane";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { OrchestrationPanel } from "@/components/workflow/orchestration-panel";
import { useWorkspaceWorkflow } from "@/components/workflow/use-workspace-workflow";
import { ChangesPanel, type ChangesSectionFilter } from "@/components/workspace/changes-panel";
import { FileTreePanel } from "@/components/workspace/file-tree-panel";
import { ReviewPanel } from "@/components/workspace/review-panel";
import { useActiveChatTasks } from "@/hooks/use-active-chat-tasks";
import { useTitlebarOverlay } from "@/hooks/use-gui-chrome";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { isMarkdownFile } from "@/components/editor/EditorPane";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { maxRightPanelWidth } from "@/lib/right-panel-width";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { useEditorStore } from "@/stores/editor-store";
import { useDiffStore } from "@/stores/diff-store";
import {
  selectShowHiddenFiles,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import {
  DEFAULT_RIGHT_PANEL_PANES,
  RIGHT_PANEL_EMPTY,
  useUIStore,
  type RightPanelCorePane,
  type RightPanelTab,
} from "@/stores/ui-store";
import {
  createTab,
  dbSetUiState,
  dockBrowserInRightPanel,
  undockBrowserFromRightPanel,
} from "@/tauri/commands";
import type {
  CheckInfo,
  InlineReviewComment,
  ReviewComment,
  WorkspaceSnapshot,
} from "@/tauri/types";

import {
  BrowserPaneActions,
  RightPanelBrowserPane,
} from "./right-panel/browser-pane";
import { DocPane, docEditorTabId } from "./right-panel/doc-pane";
import { PaneActionButton } from "./right-panel/pane-actions";
import { PanePicker } from "./right-panel/pane-picker";
import { PaneStatusFoot } from "./right-panel/pane-status-foot";
import { deckStatusLine } from "./right-panel/pane-status";
import {
  CONDITIONAL_PANES,
  PANE_REGISTRY,
  baseName,
  docPaneId,
  docPanePath,
  isCorePane,
  paneMeta,
  relativeToRoot,
} from "./right-panel/pane-registry";
import { PaneTabStrip, type DeckTab } from "./right-panel/pane-tab-strip";
import { SubagentsPane } from "./right-panel/subagents-pane";
import type { SurfaceAction } from "./right-panel/surface-actions";

interface Props {
  workspace: WorkspaceSnapshot;
  activeTab: RightPanelTab;
}

/** The width the expand toggle returns to. Its other endpoint is whatever
 *  the current layout allows — see `@/lib/right-panel-width`. */
const PANEL_DEFAULT_WIDTH = 320;

const EMPTY_MESSAGES: ChatViewItem[] = [];

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
        <span className="font-mono text-[9.5px] tabular-nums">{commentCount}</span>
      )}
      {status === "pending" && (
        <Loader2 className="size-3 animate-spin text-status-working" />
      )}
      {status === "success" && <Check className="size-3 text-status-open" />}
      {status === "failure" && <X className="size-3 text-status-attention" />}
    </>
  );
}

/** Per-doc-pane view state, kept in the deck so the shared pane bar can
 *  drive it and switching tabs doesn't reset it. */
interface DocViewState {
  raw: boolean;
  wrap: boolean;
  tree: boolean;
}

const DEFAULT_DOC_VIEW: DocViewState = { raw: false, wrap: true, tree: false };

const CHANGES_FILTER_LABEL: Record<ChangesSectionFilter, string> = {
  all: "All changes",
  staged: "Staged only",
  unstaged: "Changed only",
  conflicts: "Conflicts only",
};

// #127: memo is effective because setAppState performs structural sharing — the
// `workspace` snapshot keeps a stable ref across backend ticks that don't change
// it, and `activeTab` is a primitive, so shallow compare skips re-renders.
export const RightPanel = memo(function RightPanel({ workspace, activeTab }: Props) {
  const workspaceId = workspace.workspace_id;
  const cwd = workspace.worktree_path ?? workspace.cwd;

  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  const addRightPanelPane = useUIStore((s) => s.addRightPanelPane);
  const closeRightPanelPane = useUIStore((s) => s.closeRightPanelPane);
  const setShowFileSearch = useUIStore((s) => s.setShowFileSearch);
  const setRightPanelWidth = useUIStore((s) => s.setRightPanelWidth);
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth);
  const rightPanelRowWidth = useUIStore((s) => s.rightPanelRowWidth);
  const storedPanes = useUIStore((s) => s.rightPanelPanes[workspaceId]);
  const storedDismissed = useUIStore((s) => s.rightPanelDismissedPanes[workspaceId]);
  const showHidden = useSyncedSettingsStore(selectShowHiddenFiles);
  const updateSetting = useSyncedSettingsStore((s) => s.updateSetting);
  const { getKeysForAction } = useResolvedKeybinds();

  // Two independent reasons the tab row may need to start 40px down, and
  // both only exist while the titlebar is a floating overlay:
  //   1. the native window controls island pinned to the top-right corner
  //      (desktop only — the web client renders none), and
  //   2. the overlay's own drag layer, which is skipped on the web client
  //      precisely because it has nothing to drag (see title-bar.tsx).
  // With legacy chrome the in-flow `h-9` bar already pushes this panel
  // down, so any clearance here would be a blank band above the tabs.
  const titlebarOverlay = useTitlebarOverlay();

  const workspaceWorkflow = useWorkspaceWorkflow(workspace);
  const {
    threadId,
    tasks: activeChatTasks,
    updatedAt: tasksUpdatedAt,
    streaming: tasksThreadStreaming = false,
  } = useActiveChatTasks(workspace);
  const tasksSnapshot =
    activeChatTasks && activeChatTasks.tasks.length > 0 ? activeChatTasks : null;
  const messages = useAgentChatStore((s) =>
    threadId ? (s.threads[threadId]?.messages ?? EMPTY_MESSAGES) : EMPTY_MESSAGES,
  );
  const contextUsage = useAgentChatStore((s) =>
    threadId ? (s.threads[threadId]?.contextUsage ?? null) : null,
  );

  // The Orchestration pane appears only once a run is approved (design:
  // the approval card in the thread owns the pending_approval state; the
  // panel would just duplicate the planned phases as "queued").
  const workflowRun =
    workspaceWorkflow.run != null &&
    workspaceWorkflow.run.status !== "pending_approval"
      ? workspaceWorkflow.run
      : null;
  const workflowThreadId = workflowRun != null ? workspaceWorkflow.threadId : null;

  const subagentSummary = useMemo(() => {
    let groups = 0;
    let running = 0;
    for (const item of messages) {
      if (item.kind !== "subagent_run") continue;
      groups += 1;
      for (const view of item.subagents) {
        if (view.status === "running" || view.status === "pending") running += 1;
      }
    }
    return { groups, running };
  }, [messages]);

  // ── Deck membership ──
  const openPanes = storedPanes ?? DEFAULT_RIGHT_PANEL_PANES;
  const dismissed = storedDismissed;

  const isAvailable = useCallback(
    (id: RightPanelTab): boolean => {
      if (!isCorePane(id)) return true;
      switch (id) {
        case "tasks":
          return tasksSnapshot != null;
        case "orchestration":
          return workflowRun != null;
        case "subagents":
          return subagentSummary.groups > 0;
        default:
          return true;
      }
    },
    [tasksSnapshot, workflowRun, subagentSummary.groups],
  );

  // Availability-gated panes join the strip on their own when their data
  // shows up (that's how the Tasks tab has always behaved) — unless the
  // user closed them, which is what `rightPanelDismissedPanes` records.
  useEffect(() => {
    for (const id of CONDITIONAL_PANES) {
      if (!isAvailable(id)) continue;
      if (openPanes.includes(id)) continue;
      if (dismissed?.includes(id)) continue;
      addRightPanelPane(workspaceId, id);
    }
  }, [isAvailable, openPanes, dismissed, addRightPanelPane, workspaceId]);

  const visiblePanes = useMemo(
    () => openPanes.filter((id) => isAvailable(id)),
    [openPanes, isAvailable],
  );
  const activePane: RightPanelTab | null = visiblePanes.includes(activeTab)
    ? activeTab
    : (visiblePanes[0] ?? null);

  // ── Browser pane ──
  //
  // Opening the pane docks the workspace's one agent browser session here
  // (see `dock_browser_in_right_panel`); closing it undocks. The deck never
  // mints a browser of its own, so "is a browser open?" is always a
  // question about that single session.
  const browserSession = useAppStore(
    (s) =>
      s.appState?.agent_browser_sessions?.find(
        (abs) => abs.workspace_id === workspaceId,
      ) ?? null,
  );
  const browserOpen = visiblePanes.includes("browser");
  const browserDocked = browserSession?.right_panel_docked === true;
  const browserAttachedToPane = browserSession?.pane_id != null;
  const browserSessionName = browserSession?.cli_session_name ?? null;
  // Tracks whether the deck has actually held the session, which is what
  // separates the two reasons `browserDocked` can be false while the tab is
  // open: we haven't docked yet, or a main-area pane took the session.
  const heldBrowserRef = useRef(false);

  useEffect(() => {
    if (!browserOpen) {
      heldBrowserRef.current = false;
      return;
    }
    if (browserDocked) {
      heldBrowserRef.current = true;
      return;
    }
    if (heldBrowserRef.current && browserAttachedToPane) {
      // Something opened a browser in the main area, which re-attaches this
      // session to a pane-tree node. One session, one surface — so the deck
      // yields its tab rather than mirroring the same Chromium twice.
      heldBrowserRef.current = false;
      closeRightPanelPane(workspaceId, "browser");
      return;
    }
    // First open (or a re-open after the agent detached it). Docking adopts
    // a main-area browser pane for this session if one exists, so this is
    // also the "there's already a browser, use that one" path.
    dockBrowserInRightPanel(workspaceId).catch(console.error);
  }, [
    browserOpen,
    browserDocked,
    browserAttachedToPane,
    closeRightPanelPane,
    workspaceId,
  ]);

  // ── Per-pane view state ──
  const [docViews, setDocViews] = useState<Record<string, DocViewState>>({});
  const [changesFilter, setChangesFilter] = useState<ChangesSectionFilter>("all");
  const [treeRefreshKey, setTreeRefreshKey] = useState(0);
  const [changesRefreshKey, setChangesRefreshKey] = useState(0);
  const [copiedDoc, setCopiedDoc] = useState(false);
  const copyTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
    },
    [],
  );

  const activeDocPath = activePane ? docPanePath(activePane) : null;
  const docView = activeDocPath
    ? (docViews[activePane as string] ?? DEFAULT_DOC_VIEW)
    : DEFAULT_DOC_VIEW;
  const patchDocView = useCallback(
    (paneId: string, patch: Partial<DocViewState>) =>
      setDocViews((prev) => ({
        ...prev,
        [paneId]: { ...(prev[paneId] ?? DEFAULT_DOC_VIEW), ...patch },
      })),
    [],
  );

  // ── Deck actions ──
  // The file the tree draws as selected. The tree lost its size column and
  // its own header, so a selected row is the only thing left telling you
  // where in the tree the doc pane you just opened came from.
  const [selectedDocPath, setSelectedDocPath] = useState<string | null>(null);

  const openDocPane = useCallback(
    (filePath: string) => {
      // Seed the editor store before the pane mounts so the body never
      // flashes the editor's "open a file" empty state.
      useEditorStore.getState().initTab(docEditorTabId(workspaceId, filePath), {
        filePath,
      });
      setSelectedDocPath(filePath);
      setRightPanelTab(workspaceId, docPaneId(filePath));
    },
    [workspaceId, setRightPanelTab],
  );

  const diffTabId = `right-panel:${workspaceId}:diff`;
  const diffInitTab = useDiffStore((s) => s.initTab);
  const diffSetFile = useDiffStore((s) => s.setFile);
  const diffSetLayout = useDiffStore((s) => s.setLayout);
  const diffTab = useDiffStore((s) => s.tabs[diffTabId]);

  /** Hand the panel's diff to a full main-area tab, which has the room
   *  for hunk/file navigation and focus mode. */
  const handlePromoteDiff = useCallback(() => {
    const current = useDiffStore.getState().tabs[diffTabId];
    if (!current?.filePath) return;
    createTab(workspaceId, "diff")
      .then((tabId) =>
        diffInitTab(tabId, { file: current.filePath!, staged: current.staged }),
      )
      .catch(console.error);
  }, [diffTabId, diffInitTab, workspaceId]);

  const openDiffPane = useCallback(
    (filePath: string, staged: boolean) => {
      if (useDiffStore.getState().getTab(diffTabId)) {
        diffSetFile(diffTabId, filePath, staged);
      } else {
        diffInitTab(diffTabId, { file: filePath, staged });
      }
      setRightPanelTab(workspaceId, "diff");
    },
    [diffTabId, diffInitTab, diffSetFile, setRightPanelTab, workspaceId],
  );

  // Closing the pane you're looking at hands focus to its neighbour, and
  // closing the last one lands on the surface picker — closing a tab and
  // dismissing the column it lives in are different requests, and the
  // titlebar's panel toggle is still the one that collapses. The decision
  // is made from the *rendered* active pane, which can differ from the
  // persisted one (workspace-main coerces a stale tab without writing to
  // the store).
  const handleClose = useCallback(
    (id: RightPanelTab) => {
      closeRightPanelPane(workspaceId, id);
      if (id === "browser") {
        // Explicit user close: tell the backend the browser has no surface
        // again, and that this was deliberate so the agent's next command
        // doesn't immediately re-surface it. The Chromium keeps running —
        // same as closing a browser pane in the main area.
        undockBrowserFromRightPanel(workspaceId, true).catch(console.error);
      }
      if (id !== activePane) return;
      const index = visiblePanes.indexOf(id);
      const remaining = visiblePanes.filter((pane) => pane !== id);
      setRightPanelTab(
        workspaceId,
        remaining[Math.min(index, remaining.length - 1)] ?? RIGHT_PANEL_EMPTY,
      );
    },
    [closeRightPanelPane, setRightPanelTab, workspaceId, activePane, visiblePanes],
  );

  const handleOpenTerminal = useCallback(() => {
    createTab(workspaceId, "terminal").catch(console.error);
  }, [workspaceId]);

  const handleOpenFile = useCallback(() => {
    setShowFileSearch(true, "right-panel");
  }, [setShowFileSearch]);

  // Legacy chrome only. In GUI chrome the panel's expand control is the
  // titlebar's fixed cluster, and it is a *full* expand — the panel takes
  // the whole content row rather than snapping to the 75% width cap. This
  // width-snap stays for the in-flow `h-9` bar, where there is no titlebar
  // cluster to host the real one.
  //
  // "Expanded" is relative to what the layout currently allows, not to a
  // fixed pixel width: the panel's maximum is a share of the row it sits
  // in, so the toggle's far endpoint moves with the window. Before the row
  // has been measured the toggle still works — it just falls back to the
  // stored width so the first click can't snap the panel to a bogus size.
  const panelMaxWidth =
    rightPanelRowWidth > 0 ? maxRightPanelWidth(rightPanelRowWidth) : rightPanelWidth;
  const expanded = rightPanelWidth >= panelMaxWidth;
  const handleToggleExpand = useCallback(() => {
    const next = expanded ? PANEL_DEFAULT_WIDTH : Math.round(panelMaxWidth);
    setRightPanelWidth(next);
    dbSetUiState("right_panel_width", String(next)).catch(console.error);
  }, [expanded, panelMaxWidth, setRightPanelWidth]);

  const handleCollapsePanel = useCallback(() => {
    // A collapsed panel is not a surface. Leaving the session docked would
    // tell the backend the user can see a browser they can't — the agent
    // would then neither split a pane nor raise the background chip for it.
    // `dismissed: false`: collapsing the panel is not "close this browser",
    // so the agent may still surface it. The browser tab stays in the deck,
    // and re-opening the panel re-docks it.
    if (browserOpen) {
      undockBrowserFromRightPanel(workspaceId, false).catch(console.error);
    }
    setRightPanelTab(workspaceId, null);
  }, [browserOpen, setRightPanelTab, workspaceId]);

  const handleCopyDoc = useCallback(() => {
    if (!activeDocPath) return;
    const content =
      useEditorStore.getState().getTab(docEditorTabId(workspaceId, activeDocPath))
        ?.baselineContent ?? "";
    void navigator.clipboard
      ?.writeText(content)
      .then(() => {
        setCopiedDoc(true);
        if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
        copyTimer.current = window.setTimeout(() => setCopiedDoc(false), 1500);
      })
      .catch(() => {
        /* best-effort */
      });
  }, [activeDocPath, workspaceId]);

  // ── Strip model ──
  const tabs: DeckTab[] = visiblePanes.map((id) => {
    const path = docPanePath(id);
    if (path) {
      return { id, label: baseName(path), icon: FileText, testId: "doc-tab" };
    }
    const meta = paneMeta(id as RightPanelCorePane)!;
    const tab: DeckTab = { id, label: meta.label, icon: meta.icon };
    switch (id) {
      case "changes":
        tab.testId = "changes-tab";
        if (workspace.git_changed_files > 0) tab.badge = workspace.git_changed_files;
        break;
      case "review":
        tab.testId = "review-tab";
        if (workspace.pr_number != null) {
          tab.badge = (
            <ReviewTabBadge
              workspaceId={workspaceId}
              prNumber={workspace.pr_number}
            />
          );
        }
        break;
      case "tasks": {
        tab.testId = "tasks-tab";
        const done =
          tasksSnapshot?.tasks.filter((t) => t.status === "completed").length ?? 0;
        tab.badge = `${done}/${tasksSnapshot?.tasks.length ?? 0}`;
        tab.accentBadgeWhenActive = true;
        break;
      }
      case "subagents":
        tab.testId = "subagents-tab";
        if (subagentSummary.running > 0) tab.badge = subagentSummary.running;
        break;
      case "orchestration":
        tab.testId = "orchestration-tab";
        break;
    }
    return tab;
  });

  // One action set, two renderers: the `+` menu in the tab row and the
  // empty panel's card grid (`pane-picker.tsx`). Terminal leads because it
  // is the only entry that isn't a deck pane — it opens a real workspace
  // pane next door, exactly as the main tab strip's `+` does.
  const surfaces: SurfaceAction[] = [
    {
      id: "terminal",
      label: "Terminal",
      description: "Start a shell in this workspace.",
      icon: Terminal,
      onOpen: handleOpenTerminal,
    },
    ...PANE_REGISTRY.filter(
      (meta) => isAvailable(meta.id) && !visiblePanes.includes(meta.id),
    ).map((meta) => ({
      id: meta.id,
      label: meta.label,
      description: meta.description,
      icon: meta.icon,
      onOpen: () => setRightPanelTab(workspaceId, meta.id),
    })),
  ];

  // ── Pane action model ──
  //
  // Every pane's controls are a plain `ReactNode` rendered into the tab
  // row's right-hand slot, and they swap in place when the active tab
  // changes. There is no per-pane toolbar and no breadcrumb row: the
  // workspace name is already in the sidebar and the composer, and the
  // pane's own name is already the tab label. Anything a pane wants to say
  // about *what it is looking at* goes to the status foot below the body
  // (see `pane-status.ts`) — that is where the browser's URL and the
  // change/PR summaries live.
  let actions: ReactNode = null;

  if (activeDocPath) {
    const paneKey = activePane as string;
    actions = (
      <>
        {isMarkdownFile(activeDocPath) && (
          <PaneActionButton
            label={docView.raw ? "Show rendered" : "Show source"}
            icon={Code2}
            active={docView.raw}
            testId="doc-raw-toggle"
            onClick={() => patchDocView(paneKey, { raw: !docView.raw })}
          />
        )}
        <PaneActionButton
          label={docView.wrap ? "Disable soft wrap" : "Soft wrap"}
          icon={WrapText}
          active={docView.wrap}
          onClick={() => patchDocView(paneKey, { wrap: !docView.wrap })}
        />
        <PaneActionButton
          label={copiedDoc ? "Copied" : "Copy file"}
          icon={copiedDoc ? Check : Copy}
          onClick={handleCopyDoc}
        />
        <PaneActionButton
          label="File explorer"
          icon={PanelRightIcon}
          active={docView.tree}
          testId="doc-tree-toggle"
          onClick={() => patchDocView(paneKey, { tree: !docView.tree })}
        />
      </>
    );
  } else {
    switch (activePane) {
      case "files":
        actions = (
          <>
            <PaneActionButton
              label={showHidden ? "Hide hidden files" : "Show hidden files"}
              icon={showHidden ? EyeOff : Eye}
              active={showHidden}
              testId="files-hidden-toggle"
              onClick={() =>
                updateSetting("file_tree", "show_hidden_files", !showHidden)
              }
            />
            <PaneActionButton
              label="Refresh"
              icon={RefreshCw}
              testId="files-refresh"
              onClick={() => setTreeRefreshKey((n) => n + 1)}
            />
          </>
        );
        break;
      case "changes":
        actions = (
          <>
            <span className="shrink-0 font-mono text-[10.5px] tabular-nums text-success">
              +{workspace.git_additions}
            </span>
            <span className="shrink-0 font-mono text-[10.5px] tabular-nums text-danger">
              −{workspace.git_deletions}
            </span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label="Filter changes"
                  data-testid="changes-filter"
                  className={cn(
                    "flex size-6 shrink-0 items-center justify-center rounded-md",
                    "transition-colors duration-[120ms]",
                    changesFilter === "all"
                      ? "text-foreground/42 hover:bg-foreground/8 hover:text-foreground/80"
                      : "bg-foreground/10 text-foreground",
                  )}
                >
                  <ListFilter className="size-[13px]" strokeWidth={1.6} />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {(
                  Object.keys(CHANGES_FILTER_LABEL) as ChangesSectionFilter[]
                ).map((value) => (
                  <DropdownMenuItem
                    key={value}
                    onClick={() => setChangesFilter(value)}
                  >
                    {CHANGES_FILTER_LABEL[value]}
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
            <PaneActionButton
              label="Refresh"
              icon={RefreshCw}
              testId="changes-refresh"
              onClick={() => setChangesRefreshKey((n) => n + 1)}
            />
          </>
        );
        break;
      case "diff":
        actions = (
          <>
            <PaneActionButton
              label={diffTab?.layout === "unified" ? "Split view" : "Unified view"}
              icon={diffTab?.layout === "unified" ? Columns2 : AlignJustify}
              onClick={() =>
                diffSetLayout(
                  diffTabId,
                  diffTab?.layout === "unified" ? "split" : "unified",
                )
              }
            />
            <PaneActionButton
              label="Open in a tab"
              icon={ExternalLink}
              disabled={!diffTab?.filePath}
              onClick={handlePromoteDiff}
            />
          </>
        );
        break;
      case "browser":
        // Back/forward/reload only. The address is too long to sit honestly
        // in a shared 36px row next to the tabs, so it stays in the status
        // foot, which tracks `current_url` — the session's own field, so it
        // follows agent navigation as well as the user's.
        actions = browserSessionName ? (
          <BrowserPaneActions sessionName={browserSessionName} />
        ) : null;
        break;
      case "tasks":
        actions = tasksSnapshot ? <TasksPaneActions snapshot={tasksSnapshot} /> : null;
        break;
    }
  }

  const statusLine = deckStatusLine({
    activePane,
    paneCount: visiblePanes.length,
    agentsWorking: subagentSummary.running + (tasksThreadStreaming ? 1 : 0),
    tasks: tasksSnapshot
      ? {
          completed: tasksSnapshot.tasks.filter((t) => t.status === "completed")
            .length,
          total: tasksSnapshot.tasks.length,
          working: tasksSnapshot.tasks.filter((t) => t.status === "in_progress")
            .length,
        }
      : null,
    changes: {
      changedFiles: workspace.git_changed_files,
      additions: workspace.git_additions,
      deletions: workspace.git_deletions,
    },
    review: { prNumber: workspace.pr_number, state: workspace.pr_state },
    diff: {
      filePath: diffTab?.filePath ? relativeToRoot(diffTab.filePath, cwd) : null,
    },
    browser: browserOpen
      ? {
          docked: browserDocked,
          url: browserSession?.current_url ?? null,
          agentDriven: browserSession?.is_active === true,
        }
      : null,
  });

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden border-l border-border bg-background">
      <PaneTabStrip
        inTitlebar={titlebarOverlay}
        tabs={tabs}
        activeTab={activePane}
        onSelect={(id) => setRightPanelTab(workspaceId, id)}
        onClose={handleClose}
        actions={actions}
        surfaces={surfaces}
        onOpenFile={handleOpenFile}
        openFileKeys={getKeysForAction("fileSearch")}
        onToggleExpand={handleToggleExpand}
        expanded={expanded}
        onCollapsePanel={handleCollapsePanel}
      />

      <div className="min-h-0 flex-1 overflow-hidden">
        {activeDocPath ? (
          <DocPane
            workspace={workspace}
            filePath={activeDocPath}
            raw={docView.raw}
            wrap={docView.wrap}
            treeOpen={docView.tree}
            onOpenFile={openDocPane}
            onSearchFiles={handleOpenFile}
            treeRefreshKey={treeRefreshKey}
          />
        ) : activePane === "files" ? (
          <FileTreePanel
            workspace={workspace}
            onOpenFile={openDocPane}
            refreshKey={treeRefreshKey}
            selectedPath={selectedDocPath}
          />
        ) : activePane === "changes" ? (
          <ChangesPanel
            workspace={workspace}
            refreshKey={changesRefreshKey}
            sectionFilter={changesFilter}
            onOpenDiff={openDiffPane}
          />
        ) : activePane === "diff" ? (
          <DiffPane tabId={diffTabId} workspace={workspace} embedded />
        ) : activePane === "review" ? (
          <ReviewPanel workspace={workspace} />
        ) : activePane === "browser" ? (
          <RightPanelBrowserPane
            session={browserDocked ? browserSession : null}
            workspaceId={workspaceId}
          />
        ) : activePane === "tasks" && tasksSnapshot ? (
          <TasksPanel snapshot={tasksSnapshot} updatedAt={tasksUpdatedAt} />
        ) : activePane === "orchestration" && workflowRun ? (
          <OrchestrationPanel
            workspace={workspace}
            run={workflowRun}
            threadId={workflowThreadId}
          />
        ) : activePane === "subagents" ? (
          <SubagentsPane threadId={threadId} messages={messages} />
        ) : (
          <PanePicker surfaces={surfaces} />
        )}
      </div>

      <PaneStatusFoot
        status={statusLine}
        tokens={
          contextUsage?.total_processed_tokens ?? contextUsage?.used_tokens ?? null
        }
      />
    </div>
  );
});
