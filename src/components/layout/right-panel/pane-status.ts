/**
 * Status-foot copy for the right-panel deck.
 *
 * The foot follows the active pane: what it says is whatever that pane's
 * own numbers are, and when the pane has nothing to report it falls back
 * to the deck's own shape ("3 panes · 1 agent working"). Kept pure so the
 * wording is testable without mounting the panel.
 *
 * Rule: never invent a figure. Every branch below is fed by state the app
 * already holds — task rows, `git_changed_files`, the PR snapshot. A pane
 * with no numbers gets the deck line, not a plausible-looking placeholder.
 */
import { browserUrlCrumb } from "@/components/browser/browser-nav";
import type { RightPanelTab } from "@/stores/ui-store";

export interface DeckStatusInput {
  activePane: RightPanelTab | null;
  /** Panes open in the strip. */
  paneCount: number;
  /** Subagents currently running in the focused thread, plus the thread
   *  itself while it streams. */
  agentsWorking: number;
  tasks: { completed: number; total: number; working: number } | null;
  /** Files the git watcher reports as changed, and the working tree's
   *  net line delta. Both come straight off the workspace snapshot. */
  changes: { changedFiles: number; additions: number; deletions: number } | null;
  review: { prNumber: number | null; state: string | null } | null;
  /** The diff pane's current file, relative to the workspace root. The
   *  deck has no breadcrumb row any more, so this is where "which file am
   *  I diffing?" is answered. */
  diff: { filePath: string | null } | null;
  /** The workspace's agent browser session, when the browser pane is open.
   *  `docked` is false for the beat before the backend hands the session
   *  back; `agentDriven` is its `is_active` flag, which the backend raises
   *  while an agent is driving the browser. */
  browser: { docked: boolean; url: string | null; agentDriven: boolean } | null;
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

export function deckStatusLine(input: DeckStatusInput): string {
  const { activePane } = input;

  if (activePane === "tasks" && input.tasks) {
    const { completed, total, working } = input.tasks;
    const head = `${completed} of ${total} done`;
    return working > 0 ? `${head} · ${working} working` : head;
  }

  if (activePane === "changes" && input.changes) {
    const { changedFiles, additions, deletions } = input.changes;
    if (changedFiles === 0) return "working tree clean";
    return `${plural(changedFiles, "file")} changed · +${additions} −${deletions}`;
  }

  if (activePane === "diff" && input.diff) {
    return input.diff.filePath ?? "no file selected";
  }

  if (activePane === "review" && input.review) {
    const { prNumber, state } = input.review;
    if (prNumber == null) return "no pull request";
    return state ? `PR #${prNumber} · ${state.toLowerCase()}` : `PR #${prNumber}`;
  }

  if (activePane === "browser" && input.browser) {
    const { docked, url, agentDriven } = input.browser;
    if (!docked) return "starting browser…";
    const head = url ? browserUrlCrumb(url) : "about:blank";
    // The session is shared with the agent, and that is the one fact the
    // foot can add that the crumb above it doesn't already carry.
    return agentDriven ? `${head} · agent session` : head;
  }

  const panes = plural(input.paneCount, "pane");
  return input.agentsWorking > 0
    ? `${panes} · ${plural(input.agentsWorking, "agent")} working`
    : `${panes} · idle`;
}
