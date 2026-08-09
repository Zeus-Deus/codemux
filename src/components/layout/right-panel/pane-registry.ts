/**
 * Right-panel pane deck — the registry.
 *
 * The right panel used to be four hard-coded tabs sharing a fixed width.
 * It is now a deck: panes are *declared here*, opened and closed like
 * editor tabs, and persisted per workspace in `ui-store`. Adding a pane
 * means adding a {@link PaneMeta} entry plus a body case in
 * `right-panel.tsx` — never another `<TabsTrigger>` welded into the strip.
 *
 * Two kinds of pane id exist (see `RightPanelTab` in `ui-store`):
 *   - core panes — a fixed string (`"changes"`, `"tasks"`, …)
 *   - doc panes  — `doc:<absolute path>`, one per opened file
 *
 * Availability is separate from openness. `files`/`changes`/`review`/`diff`/
 * `browser` are always available; `tasks`, `orchestration` and `subagents`
 * only exist while the focused thread has the data behind them, and
 * auto-open when it appears (unless the user closed them — see
 * `rightPanelDismissedPanes`).
 *
 * `browser` is a real deck pane, not a jump-out: it hosts the workspace's
 * one agent browser session (see `docs/features/browser.md`), the same
 * Chromium the `codemux browser` CLI and MCP tools drive. Opening it docks
 * that session here rather than spawning a second browser.
 */
import {
  Bot,
  Folder,
  GitCompare,
  GitPullRequest,
  Globe,
  ListChecks,
  Waypoints,
  Workflow,
  type LucideIcon,
} from "lucide-react";

import type { RightPanelCorePane, RightPanelTab } from "@/stores/ui-store";

export interface PaneMeta {
  id: RightPanelCorePane;
  /** Tab label, and the label used in the `+` menu. */
  label: string;
  icon: LucideIcon;
  /** One line, shown on the empty-panel picker card. Not in the `+` menu:
   *  a menu row that already names the pane doesn't need a sentence. */
  description: string;
  /** Availability-gated panes auto-open when their data arrives. */
  conditional?: boolean;
}

export const PANE_REGISTRY: readonly PaneMeta[] = [
  {
    id: "files",
    label: "Files",
    icon: Folder,
    description: "Browse and read the workspace tree.",
  },
  {
    id: "changes",
    label: "Changes",
    icon: Waypoints,
    description: "Stage, unstage and commit your work.",
  },
  {
    id: "diff",
    label: "Diff",
    icon: GitCompare,
    description: "Read one file's changes side by side.",
  },
  {
    id: "review",
    label: "Review",
    icon: GitPullRequest,
    description: "Follow the pull request and its checks.",
  },
  {
    id: "browser",
    label: "Browser",
    icon: Globe,
    description: "Open a local app or URL beside the chat.",
  },
  {
    id: "tasks",
    label: "Tasks",
    icon: ListChecks,
    description: "Track what the agent is working through.",
    conditional: true,
  },
  {
    id: "subagents",
    label: "Subagents",
    icon: Bot,
    description: "Watch delegated agents as they run.",
    conditional: true,
  },
  {
    id: "orchestration",
    label: "Orchestration",
    icon: Workflow,
    description: "Follow the phases of a workflow run.",
    conditional: true,
  },
];

const BY_ID = new Map(PANE_REGISTRY.map((meta) => [meta.id, meta]));

export function paneMeta(id: RightPanelCorePane): PaneMeta | null {
  return BY_ID.get(id) ?? null;
}

/** Panes that only exist while their data does, in strip order. */
export const CONDITIONAL_PANES: readonly RightPanelCorePane[] =
  PANE_REGISTRY.filter((meta) => meta.conditional).map((meta) => meta.id);

const DOC_PREFIX = "doc:";

export function docPaneId(filePath: string): RightPanelTab {
  return `${DOC_PREFIX}${filePath}`;
}

/** Stable editor-store key for a file shown in a workspace's deck. */
export function docEditorTabId(workspaceId: string, filePath: string): string {
  return `right-panel:${workspaceId}:${filePath}`;
}

/** The file path behind a `doc:` pane, or null for a core pane. */
export function docPanePath(id: RightPanelTab): string | null {
  return id.startsWith(DOC_PREFIX) ? id.slice(DOC_PREFIX.length) : null;
}

export function isCorePane(id: RightPanelTab): id is RightPanelCorePane {
  return !id.startsWith(DOC_PREFIX);
}

/** Last path segment — the doc tab's label and breadcrumb leaf. */
export function baseName(filePath: string): string {
  return filePath.split(/[/\\]/).filter(Boolean).pop() ?? filePath;
}

/**
 * A file path relative to the workspace root, for the pane-bar
 * breadcrumb. Falls back to the bare basename when the file lives
 * outside the workspace (an absolute path would blow out a 240px column).
 */
export function relativeToRoot(filePath: string, root: string): string {
  const normalizedRoot = root.replace(/[/\\]+$/, "");
  if (normalizedRoot && filePath.startsWith(`${normalizedRoot}/`)) {
    return filePath.slice(normalizedRoot.length + 1);
  }
  return baseName(filePath);
}
