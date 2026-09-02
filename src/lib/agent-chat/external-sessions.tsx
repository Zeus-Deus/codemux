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

/** `/home/me/projects/app` → `~/projects/app`. A row's description is
 *  truncated from the right, so the useful tail survives only if the
 *  head is short; the home prefix carries no information worth the
 *  width. Paths outside the home directory come back unchanged. */
export function abbreviateHome(path: string, homeDir?: string | null): string {
  if (!homeDir) return path;
  const home = homeDir.replace(/[\\/]+$/, "");
  if (home.length === 0) return path;
  if (path === home) return "~";
  if (path.startsWith(`${home}/`) || path.startsWith(`${home}\\`)) {
    return `~${path.slice(home.length)}`;
  }
  return path;
}

function normalizeDir(path: string): string {
  return path.replace(/[\\/]+$/, "");
}

/** One heading per project when discovery spans the whole machine.
 *  The folder name is the label; two projects that share a name are
 *  told apart by their parent segment, so `api/app` and `web/app` do
 *  not collapse into a single bucket. */
export function projectGroupLabels(
  sessions: AdoptableAgentSession[],
): Map<string, string> {
  const byName = new Map<string, Set<string>>();
  for (const session of sessions) {
    const dir = normalizeDir(session.cwd);
    const name = externalSessionFolderLabel(dir);
    if (!byName.has(name)) byName.set(name, new Set());
    byName.get(name)!.add(dir);
  }
  const labels = new Map<string, string>();
  for (const [name, dirs] of byName) {
    for (const dir of dirs) {
      if (dirs.size === 1) {
        labels.set(dir, name);
        continue;
      }
      const parts = dir.split(/[\\/]+/).filter((part) => part.length > 0);
      const parent = parts[parts.length - 2];
      labels.set(dir, parent ? `${parent}/${name}` : name);
    }
  }
  return labels;
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
  /** The directory discovery was scoped from. A same-repo session that
   *  lives elsewhere is a sibling worktree: it still counts as this
   *  checkout, but adopting it opens that worktree's workspace, so the
   *  row must say so. */
  currentCwd?: string | null;
  /** Home directory, so paths in descriptions read `~/…`. */
  homeDir?: string | null;
  /** When discovery spans every project, one flat "other projects"
   *  bucket is a wall. Group unrelated sessions by project instead,
   *  most recently active project first. */
  groupByProject?: boolean;
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
  currentCwd = null,
  homeDir = null,
  groupByProject = false,
}: BuildAdoptableSessionItemsArgs): SlashCommandItem[] {
  const when = (session: AdoptableAgentSession) =>
    Date.parse(session.last_modified) || 0;
  const projectLabels = groupByProject
    ? projectGroupLabels(
        sessions.filter(
          (session) => adoptableSessionGroup(session) === RESUME_GROUP_OTHER,
        ),
      )
    : new Map<string, string>();
  const groupOf = (session: AdoptableAgentSession): string => {
    const base = adoptableSessionGroup(session);
    if (base !== RESUME_GROUP_OTHER || !groupByProject) return base;
    return projectLabels.get(normalizeDir(session.cwd)) ?? RESUME_GROUP_OTHER;
  };
  // Project groups are ordered by their most recent conversation, so
  // the project you were just working in sits at the top.
  const projectRecency = new Map<string, number>();
  for (const session of sessions) {
    const group = groupOf(session);
    if (GROUP_ORDER.includes(group)) continue;
    projectRecency.set(
      group,
      Math.max(projectRecency.get(group) ?? 0, when(session)),
    );
  }
  const groupRank = (group: string): number => {
    const fixed = GROUP_ORDER.indexOf(group);
    return fixed === -1 ? GROUP_ORDER.length : fixed;
  };
  const sorted = [...sessions].sort((a, b) => {
    const ga = groupOf(a);
    const gb = groupOf(b);
    const rankDelta = groupRank(ga) - groupRank(gb);
    if (rankDelta !== 0) return rankDelta;
    if (ga !== gb) {
      return (projectRecency.get(gb) ?? 0) - (projectRecency.get(ga) ?? 0);
    }
    return when(b) - when(a);
  });

  const here = currentCwd ? normalizeDir(currentCwd) : null;
  return sorted.map((session) => {
    const group = groupOf(session);
    const base = adoptableSessionGroup(session);
    const existing = base === RESUME_GROUP_EXISTING;
    const foreign = base === RESUME_GROUP_OTHER;
    const siblingWorktree =
      !existing &&
      !foreign &&
      here !== null &&
      normalizeDir(session.cwd) !== here;
    const where = abbreviateHome(session.cwd, homeDir);
    return {
      id: externalSessionRowId(session.session_id),
      label: session.title,
      description: existing
        ? "Already in Codemux — switches to that thread"
        : foreign
          ? foreignNeedsConfirm
            ? `Asks first, then opens in ${where}`
            : `Opens in ${where}`
          : siblingWorktree
            ? `Worktree · ${where}`
            : (session.git_branch ?? where),
      // No literal text ever reaches the draft, so the command slot is
      // free for a state word. `rightAdornment` overrides it anyway;
      // this is the fallback for surfaces that render `command` only.
      command: existing
        ? "switch"
        : foreign
          ? "other project"
          : siblingWorktree
            ? "worktree"
            : "adopt",
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
