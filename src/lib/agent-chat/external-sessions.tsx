import { relativeTime } from "@/lib/relative-time";
import type {
  AdoptableAgentSession,
  ExternalAgentSession,
} from "@/tauri/commands";

/**
 * The `/resume` picker's data model: conversations the agent's own CLI
 * created OUTSIDE Codemux, offered for adoption into a thread.
 *
 * Every session on the machine is discovered at once and grouped by the
 * project it ran in (linked worktrees fold into their repository). The
 * picker opens the selected project and collapses everything else, so
 * the collapsed folders ARE the wider scope — there is no separate
 * "search every project" toggle. Picking a session from another project
 * moves the chat there; the footer sentence built by
 * {@link describeResumeDestination} is the confirmation.
 *
 * Kept apart from `slash-commands.ts` because these rows are entity
 * results (one per discovered conversation) rather than a fixed command
 * vocabulary, and because the grouping and destination rules encode
 * product decisions worth their own unit tests.
 */

/** Folder key for sessions that ran outside any git repository. */
export const RESUME_HOME_FOLDER_KEY = "home";
/** Display name of that bucket (upper-cased by the heading style). */
export const RESUME_HOME_FOLDER_NAME = "Home folder";
/** How many rows the Home draft's RECENT block shows. */
export const RESUME_RECENT_COUNT = 3;

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

/** Last path segment of a directory, for folder names. Tolerates
 *  trailing separators and both separator styles so a Windows path
 *  reads the same way. */
export function externalSessionFolderLabel(cwd: string): string {
  const parts = cwd.split(/[\\/]+/).filter((part) => part.length > 0);
  return parts[parts.length - 1] ?? cwd;
}

/** `/home/me/projects/app` → `~/projects/app`. The home prefix carries
 *  no information worth the width. Paths outside the home directory
 *  come back unchanged. */
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
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.length > 0 ? trimmed : path;
}

/** Folder key a session files under: its canonical project root, or
 *  the home bucket when it ran outside any repository. */
export function resumeFolderKey(session: AdoptableAgentSession): string {
  return session.project_root
    ? normalizeDir(session.project_root)
    : RESUME_HOME_FOLDER_KEY;
}

function lastModifiedMs(session: AdoptableAgentSession): number {
  return Date.parse(session.last_modified) || 0;
}

/** One picker row. */
export interface ResumeRow {
  id: string;
  session: AdoptableAgentSession;
  folderKey: string;
  /** Folder name of the project, or {@link RESUME_HOME_FOLDER_NAME}. */
  project: string;
  isHome: boolean;
  /** Codemux already owns a thread for it — the pick switches. */
  alreadyOpen: boolean;
  /** Relative "6 days ago" label. */
  when: string;
}

/** One project (or the home bucket) in the picker. */
export interface ResumeFolder {
  key: string;
  /** Folder name, e.g. `codemux`; {@link RESUME_HOME_FOLDER_NAME} for home. */
  name: string;
  /** `~`-relative path of the project root; `~` for the home bucket. */
  path: string;
  /** The project the draft / pane is on — rendered open, with a badge. */
  isSelected: boolean;
  isHome: boolean;
  count: number;
  /** Rows whose cwd is a linked worktree of this project. */
  worktreeCount: number;
  newestMs: number;
  /** Relative label of the newest row. */
  newest: string;
  /** Most recently touched first. */
  rows: ResumeRow[];
}

export interface ResumeGrouping {
  /** Every discovered session, before any search filter. */
  total: number;
  /** The Home draft's cross-project RECENT block; null when a project is
   *  selected or a search is active. */
  recent: ResumeRow[] | null;
  /** Selected project first, then by newest session, home bucket last.
   *  While a search is active, folders without a match are omitted. */
  folders: ResumeFolder[];
}

interface GroupAdoptableSessionsArgs {
  sessions: AdoptableAgentSession[];
  /** Root of the project the surface is on; null for a Home draft. */
  selectedProjectRoot: string | null;
  /** Home directory, so folder paths read `~/…`. */
  homeDir?: string | null;
  /** Search text; matched against title, branch, project and worktree. */
  query?: string;
  /** Injected for deterministic relative-time assertions. */
  now?: Date;
}

function rowMatches(row: ResumeRow, needle: string): boolean {
  const { session } = row;
  const haystack = [
    session.title,
    session.git_branch ?? "",
    session.worktree_name ?? "",
    row.project,
    session.cwd,
  ];
  return haystack.some((part) => part.toLowerCase().includes(needle));
}

/**
 * Group discovered sessions into the picker's folders.
 *
 * Linked worktrees fold into their repository's folder (the backend
 * resolves `project_root` to the main root) and are counted separately
 * so a collapsed line can say "5 · 2 in worktrees". Sessions with no
 * project root share one home bucket, listed last.
 */
export function groupAdoptableSessions({
  sessions,
  selectedProjectRoot,
  homeDir = null,
  query = "",
  now = new Date(),
}: GroupAdoptableSessionsArgs): ResumeGrouping {
  const selectedKey = selectedProjectRoot
    ? normalizeDir(selectedProjectRoot)
    : null;
  const needle = query.trim().toLowerCase();

  const sorted = [...sessions].sort(
    (a, b) => lastModifiedMs(b) - lastModifiedMs(a),
  );
  const allRows: ResumeRow[] = sorted.map((session) => {
    const folderKey = resumeFolderKey(session);
    const isHome = folderKey === RESUME_HOME_FOLDER_KEY;
    return {
      id: externalSessionRowId(session.session_id),
      session,
      folderKey,
      project: isHome
        ? RESUME_HOME_FOLDER_NAME
        : externalSessionFolderLabel(folderKey),
      isHome,
      alreadyOpen: session.existing_thread_id !== null,
      when: externalSessionWhen(session, now),
    };
  });
  const rows = needle
    ? allRows.filter((row) => rowMatches(row, needle))
    : allRows;

  const byKey = new Map<string, ResumeFolder>();
  for (const row of rows) {
    let folder = byKey.get(row.folderKey);
    if (!folder) {
      folder = {
        key: row.folderKey,
        name: row.project,
        path: row.isHome ? "~" : abbreviateHome(row.folderKey, homeDir),
        isSelected: selectedKey !== null && row.folderKey === selectedKey,
        isHome: row.isHome,
        count: 0,
        worktreeCount: 0,
        newestMs: lastModifiedMs(row.session),
        newest: row.when,
        rows: [],
      };
      byKey.set(row.folderKey, folder);
    }
    folder.count += 1;
    if (row.session.worktree_name) folder.worktreeCount += 1;
    // Rows arrive newest-first, so the first row set `newest` already.
    folder.rows.push(row);
  }

  const folders = [...byKey.values()].sort((a, b) => {
    if (a.isSelected !== b.isSelected) return a.isSelected ? -1 : 1;
    if (a.isHome !== b.isHome) return a.isHome ? 1 : -1;
    return b.newestMs - a.newestMs;
  });

  const recent =
    selectedKey === null && !needle && rows.length > 0
      ? rows.slice(0, RESUME_RECENT_COUNT)
      : null;

  return { total: sessions.length, recent, folders };
}

// ─── Footer: where a pick takes the chat ───────────────────────────

export type ResumeDestinationKind =
  /** Already in Codemux — switches to that thread. */
  | "switch"
  /** Same project, and a workspace is already open at the session's folder. */
  | "continue-open"
  /** Same project with nothing open there, or any pick from a Home draft. */
  | "open"
  /** Another project's main checkout — the chat moves there. */
  | "move"
  /** Another project's linked worktree — the chat moves into it. */
  | "move-worktree";

export interface ResumeDestination {
  kind: ResumeDestinationKind;
  /** Folder name; null means the home folder. */
  project: string | null;
  /** Branch (or worktree name when the branch is unknown). */
  branch: string | null;
  worktreeName: string | null;
}

interface DescribeResumeDestinationContext {
  /** Root of the project the surface is on; null for a Home draft. */
  selectedProjectRoot: string | null;
  /** Whether a workspace is already open at the session's own cwd. */
  workspaceOpen: boolean;
}

/** Classify what pressing Enter on `row` does, for the footer. */
export function describeResumeDestination(
  row: ResumeRow,
  { selectedProjectRoot, workspaceOpen }: DescribeResumeDestinationContext,
): ResumeDestination {
  const { session } = row;
  const project = row.isHome ? null : row.project;
  const worktreeName = session.worktree_name;
  const branch = session.git_branch ?? worktreeName;
  const base = { project, branch, worktreeName };
  if (row.alreadyOpen) return { kind: "switch", ...base };
  if (selectedProjectRoot === null) return { kind: "open", ...base };
  if (row.folderKey === normalizeDir(selectedProjectRoot)) {
    return { kind: workspaceOpen ? "continue-open" : "open", ...base };
  }
  return { kind: worktreeName ? "move-worktree" : "move", ...base };
}

export interface ResumeDestinationSegment {
  text: string;
  /** Rendered in the mono face (project, branch and worktree names). */
  mono?: boolean;
  /** Rendered dimmer (the parenthetical). */
  dim?: boolean;
}

function placeSegments(dest: ResumeDestination): ResumeDestinationSegment[] {
  if (dest.project === null) return [{ text: "your home folder" }];
  const out: ResumeDestinationSegment[] = [{ text: dest.project, mono: true }];
  if (dest.branch) out.push({ text: " · " }, { text: dest.branch, mono: true });
  return out;
}

/** The footer sentence as styled segments. */
export function resumeDestinationSegments(
  dest: ResumeDestination,
): ResumeDestinationSegment[] {
  switch (dest.kind) {
    case "switch":
      return [{ text: "Switches to the open chat in " }, ...placeSegments(dest)];
    case "continue-open":
      return [
        { text: "Continues in " },
        ...placeSegments(dest),
        { text: " — the workspace that's already open" },
      ];
    case "open":
      return [
        { text: "Opens " },
        ...placeSegments(dest),
        { text: " and continues there" },
      ];
    case "move":
      return [{ text: "Moves this chat to " }, ...placeSegments(dest)];
    case "move-worktree":
      return [
        { text: "Moves this chat to " },
        { text: dest.project ?? "", mono: true },
        { text: " → worktree " },
        { text: dest.worktreeName ?? "", mono: true },
        { text: " " },
        { text: "(already on disk, opened as a workspace)", dim: true },
      ];
  }
}

/** The footer sentence as plain text. */
export function resumeDestinationText(dest: ResumeDestination): string {
  return resumeDestinationSegments(dest)
    .map((segment) => segment.text)
    .join("");
}
