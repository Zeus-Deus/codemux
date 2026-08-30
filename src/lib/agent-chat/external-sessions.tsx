import { ArrowLeftRight, FolderOpen, Globe, Terminal } from "lucide-react";

import { relativeTime } from "@/lib/relative-time";
import type {
  AdoptableAgentSession,
  ExternalAgentSession,
} from "@/tauri/commands";

import type { SlashCommandItem } from "./slash-commands";

/**
 * Rows for the `/resume` picker: conversations the agent's own CLI
 * created OUTSIDE Codemux, offered for adoption into a thread.
 *
 * Kept apart from `slash-commands.ts` because these rows are entity
 * results (one per discovered conversation) rather than a fixed command
 * vocabulary — and because the grouping rules encode product decisions
 * (adopt in place / switch instead of re-adopting / never silently jump
 * to another project) that deserve their own unit tests.
 */

/** Sessions rooted in the checkout the pane is already pointed at.
 *  Adopting one attaches to that folder — no worktree is created. */
export const RESUME_GROUP_CHECKOUT = "THIS CHECKOUT";
/** Sessions Codemux already owns a thread for. These switch. */
export const RESUME_GROUP_EXISTING = "ALREADY IN CODEMUX";
/** Sessions from an unrelated project. Adopting one attaches to THAT
 *  project's folder, so the row spells the directory out and the pick
 *  is confirmed before anything moves — the pane is never quietly
 *  re-pointed. */
export const RESUME_GROUP_OTHER = "OTHER PROJECTS";
/** The single row that widens discovery past the current checkout. */
export const RESUME_GROUP_SCOPE = "SCOPE";

/** Id of the widen-scope row. Handled by the composer, not by adoption. */
export const RESUME_WIDEN_SCOPE_ITEM_ID = "external-session:widen-scope";

/** Stable picker-row id for one discovered session. */
export function externalSessionRowId(sessionId: string): string {
  return `external-session:${sessionId}`;
}

/** Strip the Codemux-side decorations back to the payload the adopt
 *  command takes. The extra keys would be ignored on the wire, but the
 *  command's contract is `ExternalSession` — send exactly that. */
export function toExternalAgentSession(
  session: AdoptableAgentSession,
): ExternalAgentSession {
  return {
    session_id: session.session_id,
    title: session.title,
    cwd: session.cwd,
    git_branch: session.git_branch,
    last_modified: session.last_modified,
    created_at: session.created_at,
    file_size: session.file_size,
    title_source: session.title_source,
  };
}

/** Which group a discovered session belongs to. "Already in Codemux"
 *  wins over locality: a row Codemux already owns must never offer a
 *  second adoption, wherever it lives. */
export function adoptableSessionGroup(session: AdoptableAgentSession): string {
  if (session.existing_thread_id) return RESUME_GROUP_EXISTING;
  return session.same_repo ? RESUME_GROUP_CHECKOUT : RESUME_GROUP_OTHER;
}

/** Relative "3 hours ago" label for a session's last transcript write.
 *  Unparseable timestamps degrade to "Earlier" rather than "Invalid
 *  Date" — the backend normalises to ISO-8601, but a picker row is not
 *  worth crashing over. */
export function externalSessionWhen(
  session: AdoptableAgentSession,
  now: Date = new Date(),
): string {
  const ms = Date.parse(session.last_modified);
  return Number.isFinite(ms) ? relativeTime(new Date(ms), now) : "Earlier";
}

/** Last path segment of the session's directory, for the compact
 *  right-hand adornment. Tolerates trailing separators and both
 *  separator styles so a Windows path reads the same way. */
export function externalSessionFolderLabel(cwd: string): string {
  const parts = cwd.split(/[\\/]+/).filter((part) => part.length > 0);
  return parts[parts.length - 1] ?? cwd;
}

const GROUP_ORDER = [
  RESUME_GROUP_CHECKOUT,
  RESUME_GROUP_EXISTING,
  RESUME_GROUP_OTHER,
];

interface BuildAdoptableSessionItemsArgs {
  sessions: AdoptableAgentSession[];
  /** Injected for deterministic relative-time assertions. */
  now?: Date;
  /** Whether picking an unrelated project's session asks before
   *  anything moves (a live pane would be re-pointed) — the default —
   *  or opens straight away in that session's own folder (a workspace
   *  draft has no committed location to protect). Only the row's
   *  description changes; the caller enforces whichever it promised. */
  foreignNeedsConfirm?: boolean;
}

/**
 * Build the picker rows, ordered: adoptable sessions from this checkout
 * first (the common case), then the ones Codemux already owns, then
 * unrelated projects last. Within a group, most recently touched first.
 *
 * `onSelect` is a no-op: dispatch happens in the composer, which owns
 * the session list and the pane callbacks — a per-item closure would
 * capture a stale list the same way the mention rows would.
 */
export function buildAdoptableSessionItems({
  sessions,
  now = new Date(),
  foreignNeedsConfirm = true,
}: BuildAdoptableSessionItemsArgs): SlashCommandItem[] {
  const sorted = [...sessions].sort((a, b) => {
    const groupDelta =
      GROUP_ORDER.indexOf(adoptableSessionGroup(a)) -
      GROUP_ORDER.indexOf(adoptableSessionGroup(b));
    if (groupDelta !== 0) return groupDelta;
    return (
      (Date.parse(b.last_modified) || 0) - (Date.parse(a.last_modified) || 0)
    );
  });

  return sorted.map((session) => {
    const group = adoptableSessionGroup(session);
    const existing = group === RESUME_GROUP_EXISTING;
    const foreign = group === RESUME_GROUP_OTHER;
    return {
      id: externalSessionRowId(session.session_id),
      label: session.title,
      description: existing
        ? "Already in Codemux — switches to that thread"
        : foreign
          ? foreignNeedsConfirm
            ? `Asks first, then opens in ${session.cwd}`
            : `Opens in ${session.cwd}`
          : (session.git_branch ?? session.cwd),
      // No literal text ever reaches the draft, so the command slot is
      // free for a state word. `rightAdornment` overrides it anyway;
      // this is the fallback for surfaces that render `command` only.
      command: existing ? "switch" : foreign ? "other project" : "adopt",
      icon: existing ? ArrowLeftRight : foreign ? FolderOpen : Terminal,
      iconClassName: existing
        ? "text-status-remote"
        : foreign
          ? "text-muted-foreground"
          : "text-warning",
      group,
      stacked: true,
      rightAdornment: (
        <span className="flex h-full min-w-24 flex-col items-end justify-center leading-none">
          <span className="text-[10px] font-medium text-foreground/70">
            {session.git_branch ?? externalSessionFolderLabel(session.cwd)}
          </span>
          <span className="mt-1 whitespace-nowrap font-mono text-[9px] text-muted-foreground/60">
            {externalSessionWhen(session, now)}
          </span>
        </span>
      ),
      onSelect: () => {},
    };
  });
}

/** The opt-in widening row (R3): discovery defaults to the current
 *  checkout plus its worktrees, and only reaches every project when the
 *  user asks for it. */
export function buildWidenScopeItem(): SlashCommandItem {
  return {
    id: RESUME_WIDEN_SCOPE_ITEM_ID,
    label: "Search every project",
    description: "Look past this checkout at all conversations on this machine",
    command: "all projects",
    icon: Globe,
    group: RESUME_GROUP_SCOPE,
    onSelect: () => {},
  };
}
