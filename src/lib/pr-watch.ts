/**
 * Pull-request watch: a chat is woken when its pull request's checks fail,
 * someone else comments or reviews, or the branch conflicts with its base
 * (`src-tauri/src/pr_watch.rs`). These helpers decide which chat a
 * workspace-level control acts for and how a watch reads at a glance.
 */
import { normalizePrState } from "@/components/github/pr-status-icon";
import type { PaneNodeSnapshot, PrWatchStatus, WorkspaceSnapshot } from "@/tauri/types";

export type PrWatchTone = "off" | "on" | "warning" | "error" | "muted";

export interface PrWatchView {
  /** Short trailing text: "Off", "On", "On · 1/3", "Stopped", … */
  label: string;
  tone: PrWatchTone;
  /** Tooltip: what the watch does, or why it stopped. */
  title: string;
  /** Whether a click stops the watch (otherwise it starts or re-arms it). */
  active: boolean;
}

const WHAT_IT_DOES =
  "Wakes this chat when a check fails, someone else comments or reviews, or the branch conflicts.";

export function describePrWatch(watch: PrWatchStatus | null): PrWatchView {
  if (!watch) {
    return { label: "Off", tone: "off", title: `${WHAT_IT_DOES} Click to start.`, active: false };
  }
  const pr = `#${watch.pr_number}`;
  if (watch.status === "exhausted") {
    return {
      label: "Stopped",
      tone: "warning",
      title: `${watch.stop_reason ?? "Stopped watching"} on ${pr}. Click to watch again.`,
      active: false,
    };
  }
  if (watch.status === "ended") {
    return {
      label: "Ended",
      tone: "muted",
      title: `${watch.stop_reason ?? "Stopped watching"}. Click to watch again.`,
      active: false,
    };
  }
  if (watch.last_error) {
    return {
      label: "Error",
      tone: "error",
      title: `Watching ${pr}, but the last check failed: ${watch.last_error}. Click to stop.`,
      active: true,
    };
  }
  const used = `${watch.attempts} of ${watch.max_attempts} automatic fix attempts used`;
  const last = watch.last_wake_summary ? ` Last update: ${watch.last_wake_summary}.` : "";
  return {
    label: watch.attempts > 0 ? `On · ${watch.attempts}/${watch.max_attempts}` : "On",
    tone: "on",
    title: `Watching ${pr}: ${WHAT_IT_DOES} ${used}.${last} Click to stop.`,
    active: true,
  };
}

export const PR_WATCH_TONE_CLASS: Record<PrWatchTone, string> = {
  off: "text-muted-foreground",
  on: "text-success",
  warning: "text-warning",
  error: "text-danger",
  muted: "text-muted-foreground",
};

/** An open pull request on a local GitHub checkout: what a watch can follow. */
export function canWatchPr(workspace: WorkspaceSnapshot): boolean {
  const state = normalizePrState(workspace.pr_state);
  if (state !== "open" && state !== "draft") return false;
  if (!workspace.pr_number) return false;
  if (workspace.host_id != null || workspace.attach_only) return false;
  return workspace.provider_kind == null || workspace.provider_kind === "github";
}

function chatThreads(node: PaneNodeSnapshot, out: string[]): string[] {
  if (node.kind === "agent_chat") {
    if (node.thread_id) out.push(node.thread_id);
  } else if (node.kind === "split") {
    for (const child of node.children) chatThreads(child, out);
  }
  return out;
}

/** Every chat in the workspace, across its surfaces and splits. */
export function workspaceChatThreadIds(workspace: WorkspaceSnapshot): string[] {
  const out: string[] = [];
  for (const surface of workspace.surfaces) chatThreads(surface.root, out);
  return out;
}

/**
 * The chat a workspace-level watch control acts for: the focused chat,
 * else the first chat on the active surface, else the first chat anywhere
 * in the workspace. `null` when the workspace has no chat.
 */
export function watchTargetThreadId(
  workspace: WorkspaceSnapshot,
  focusedThreadId: string | null,
): string | null {
  if (focusedThreadId) return focusedThreadId;
  const active = workspace.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
  const ordered = active
    ? [active, ...workspace.surfaces.filter((s) => s !== active)]
    : workspace.surfaces;
  for (const surface of ordered) {
    const [first] = chatThreads(surface.root, []);
    if (first) return first;
  }
  return null;
}
