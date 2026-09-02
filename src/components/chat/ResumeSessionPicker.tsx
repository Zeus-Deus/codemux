import { Command as CommandPrimitive } from "cmdk";
import {
  ArrowLeftRight,
  ChevronDown,
  ChevronRight,
  Search,
  Terminal,
} from "lucide-react";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";

import { cn } from "@/lib/utils";
import { agentDisplayName } from "@/lib/agent-chat/agent-display-name";
import {
  describeResumeDestination,
  groupAdoptableSessions,
  resumeDestinationSegments,
  type ResumeFolder,
  type ResumeRow,
} from "@/lib/agent-chat/external-sessions";
import type { AdoptableAgentSession } from "@/tauri/commands";

interface Props {
  /** Hide / show. */
  open: boolean;
  /** Every discovered session on this machine, unfiltered. */
  sessions: AdoptableAgentSession[];
  /** Provider the sessions belong to; names the agent on each row. */
  provider: string;
  /** Root of the project the surface is on — rendered open, with a
   *  badge. Null for a Home draft, which shows a RECENT block instead. */
  selectedProjectRoot: string | null;
  /** Home directory, so folder paths read `~/…`. */
  homeDir: string | null;
  /** Whether a workspace is already open at a session's folder. Decides
   *  between "continues in the open workspace" and "opens" wording. */
  isWorkspaceOpenAt: (cwd: string) => boolean;
  /** Search box value + change reporter, controlled by the composer. */
  query: string;
  onQueryChange: (next: string) => void;
  /** Discovery state, reported in the footer. */
  loading: boolean;
  error: string | null;
  /** Activated on click OR Enter on a session row. */
  onSelect: (session: AdoptableAgentSession) => void;
  /** Escape handler. */
  onEscape: () => void;
  /** Injected clock for deterministic relative times in tests. */
  now?: Date;
}

const FOLDER_VALUE_PREFIX = "folder:";
/** RECENT rows get their own cmdk value so a session that also sits in
 *  an open folder below is never highlighted twice. */
const RECENT_VALUE_PREFIX = "recent:";

const HEADING_CLASSES = cn(
  "font-mono text-[9.5px] font-semibold uppercase tracking-[0.08em]",
  "text-muted-foreground",
);

/**
 * The `/resume` picker: terminal sessions grouped by project, the
 * selected project open and every other folder one collapsed line.
 * Same surface family as {@link ComposerCommandMenu} (search header,
 * cmdk keyboard nav, tone-coloured icons); differs in that its rows are
 * two-line session cards and its footer spells out where the pick takes
 * the chat, which is the confirmation — there is no dialog after it.
 */
export function ResumeSessionPicker({
  open,
  sessions,
  provider,
  selectedProjectRoot,
  homeDir,
  isWorkspaceOpenAt,
  query,
  onQueryChange,
  loading,
  error,
  onSelect,
  onEscape,
  now,
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  // Which folders are unfolded. `null` is the default on every open:
  // only the selected project. The first toggle materialises the set.
  const [expanded, setExpanded] = useState<ReadonlySet<string> | null>(null);
  const [highlighted, setHighlighted] = useState("");

  // Reclaim focus on open. Clicking a folder line blurs the input;
  // re-focusing keeps cmdk's keyboard nav live.
  useEffect(() => {
    if (!open) return;
    const raf = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(raf);
  }, [open]);

  // Every open starts with only the selected project unfolded and cmdk
  // free to highlight the first row. Reset on CLOSE: the cmdk tree
  // remounts on the next open, and a reset on open would race its
  // first-item pick.
  useEffect(() => {
    if (open) return;
    setExpanded(null);
    setHighlighted("");
  }, [open]);

  const grouping = useMemo(
    () =>
      groupAdoptableSessions({
        sessions,
        selectedProjectRoot,
        homeDir,
        query,
        now,
      }),
    [sessions, selectedProjectRoot, homeDir, query, now],
  );
  const searching = query.trim().length > 0;

  const rowsByValue = useMemo(() => {
    const map = new Map<string, ResumeRow>();
    for (const row of grouping.recent ?? []) {
      map.set(`${RECENT_VALUE_PREFIX}${row.id}`, row);
    }
    for (const folder of grouping.folders) {
      for (const row of folder.rows) map.set(row.id, row);
    }
    return map;
  }, [grouping]);

  if (!open) return null;

  // A search opens every folder that still has a match.
  const isExpanded = (folder: ResumeFolder) =>
    searching || (expanded ? expanded.has(folder.key) : folder.isSelected);
  const toggleFolder = (folder: ResumeFolder) => {
    const opening = !isExpanded(folder);
    // Land the highlight where the eye goes — the folder's first session
    // when it opens, the folder line when it closes — and commit that
    // BEFORE the items change. cmdk re-selects its first item whenever
    // the highlighted item unmounts, so the folder line (expand) or the
    // row (collapse) must already be un-highlighted when it goes.
    flushSync(() => {
      setHighlighted(
        opening
          ? (folder.rows[0]?.id ?? `${FOLDER_VALUE_PREFIX}${folder.key}`)
          : `${FOLDER_VALUE_PREFIX}${folder.key}`,
      );
    });
    setExpanded((prev) => {
      const next = new Set(
        prev ??
          grouping.folders.filter((f) => f.isSelected).map((f) => f.key),
      );
      if (opening) next.add(folder.key);
      else next.delete(folder.key);
      return next;
    });
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const highlightedRow = rowsByValue.get(highlighted) ?? null;
  const highlightedFolder = highlighted.startsWith(FOLDER_VALUE_PREFIX)
    ? grouping.folders.find(
        (folder) => folder.key === highlighted.slice(FOLDER_VALUE_PREFIX.length),
      ) ?? null
    : null;

  const renderRow = (row: ResumeRow, recent: boolean) => {
    const { session } = row;
    const Icon = row.alreadyOpen ? ArrowLeftRight : Terminal;
    const worktree = session.worktree_name;
    const branch = session.git_branch;
    const dot = <span className="opacity-50">·</span>;
    const value = recent ? `${RECENT_VALUE_PREFIX}${row.id}` : row.id;
    return (
      <CommandPrimitive.Item
        key={value}
        value={value}
        onSelect={() => onSelect(session)}
        data-testid={`slash-item-${row.id}`}
        data-recent={recent || undefined}
        className={cn(
          "flex items-start gap-2.5 rounded-md px-2 py-1.5",
          "cursor-pointer outline-none select-none",
          "data-[selected=true]:bg-muted/70",
        )}
      >
        <Icon
          className={cn(
            "mt-0.5 h-3.5 w-3.5 shrink-0",
            row.alreadyOpen ? "text-status-remote" : "text-warning",
          )}
          aria-hidden
        />
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="line-clamp-2 break-words text-[12.5px] leading-[1.35] text-foreground">
            {session.title}
          </span>
          <span className="flex flex-wrap items-center gap-1.5 font-mono text-[9.5px] leading-none text-muted-foreground">
            {row.alreadyOpen ? (
              <span className="text-status-remote">
                already open in Codemux — switches to it
              </span>
            ) : (
              <>
                {recent && (
                  <>
                    <span>{row.project}</span>
                    {dot}
                  </>
                )}
                {worktree ? (
                  <>
                    <span className="rounded bg-muted/40 px-1 py-px text-foreground/80">
                      ⑃ worktree {worktree}
                    </span>
                    {dot}
                  </>
                ) : branch ? (
                  <>
                    <span>{branch}</span>
                    {dot}
                  </>
                ) : null}
                <span>{agentDisplayName(provider)}</span>
              </>
            )}
          </span>
        </span>
        <span className="mt-0.5 shrink-0 whitespace-nowrap font-mono text-[9.5px] text-muted-foreground">
          {row.when}
        </span>
      </CommandPrimitive.Item>
    );
  };

  const renderFolderLine = (folder: ResumeFolder, openNow: boolean) => {
    const Chevron = openNow ? ChevronDown : ChevronRight;
    const summary = [
      String(folder.count),
      folder.worktreeCount > 0 ? `${folder.worktreeCount} in worktrees` : null,
      folder.newest,
    ]
      .filter((part): part is string => part !== null)
      .join(" · ");
    const content = (
      <>
        <Chevron className="h-2.5 w-2.5 shrink-0" aria-hidden />
        <span className="shrink-0 text-foreground/80">{folder.name}</span>
        <span className="truncate normal-case tracking-normal opacity-60">
          {folder.path}
          {folder.isHome && " · ran outside any project"}
        </span>
        {folder.isSelected && (
          <span className="ml-auto shrink-0 rounded-[4px] bg-foreground/[0.06] px-1.5 py-px text-[8.5px] tracking-[0.04em]">
            Selected project
          </span>
        )}
        {!openNow && (
          <span
            className={cn(
              "shrink-0 whitespace-nowrap normal-case tracking-normal",
              !folder.isSelected && "ml-auto",
            )}
          >
            {summary}
          </span>
        )}
      </>
    );
    const testId = `resume-folder-${folder.key}`;
    if (openNow) {
      // An open folder's heading is a plain heading, not a cmdk item,
      // so the first arrow-key stop (and the footer's default subject)
      // is the first session, never a "collapse this" control. It is
      // still clickable to fold the group back up.
      return (
        <div
          key={`${FOLDER_VALUE_PREFIX}${folder.key}`}
          role="button"
          tabIndex={-1}
          onClick={() => toggleFolder(folder)}
          data-testid={testId}
          data-expanded
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 pt-2 pb-1",
            "cursor-pointer select-none hover:bg-muted/40",
            HEADING_CLASSES,
          )}
        >
          {content}
        </div>
      );
    }
    return (
      <CommandPrimitive.Item
        key={`${FOLDER_VALUE_PREFIX}${folder.key}`}
        value={`${FOLDER_VALUE_PREFIX}${folder.key}`}
        onSelect={() => toggleFolder(folder)}
        data-testid={testId}
        className={cn(
          "flex items-center gap-1.5 rounded-md px-2 py-2",
          "cursor-pointer outline-none select-none",
          "data-[selected=true]:bg-muted/70",
          HEADING_CLASSES,
        )}
      >
        {content}
      </CommandPrimitive.Item>
    );
  };

  const footer = (() => {
    if (error) {
      return {
        tone: "error" as const,
        content: <span>Resume: {error}</span>,
      };
    }
    if (loading) {
      return { tone: "muted" as const, content: <span>Reading local history…</span> };
    }
    if (grouping.total === 0) {
      return {
        tone: "muted" as const,
        content: <span>No terminal sessions found on this machine.</span>,
      };
    }
    if (highlightedRow) {
      const destination = describeResumeDestination(highlightedRow, {
        selectedProjectRoot,
        workspaceOpen: isWorkspaceOpenAt(highlightedRow.session.cwd),
      });
      return {
        tone: "muted" as const,
        content: (
          <>
            <EnterKey />
            <span data-testid="resume-destination">
              {resumeDestinationSegments(destination).map((segment, i) => (
                <span
                  key={i}
                  className={cn(
                    segment.mono && "font-mono text-foreground",
                    segment.dim && "text-muted-foreground/70",
                  )}
                >
                  {segment.text}
                </span>
              ))}
            </span>
          </>
        ),
      };
    }
    if (highlightedFolder) {
      return {
        tone: "muted" as const,
        content: (
          <>
            <EnterKey />
            <span>
              Expands{" "}
              <span className="font-mono text-foreground">
                {highlightedFolder.name}
              </span>
            </span>
          </>
        ),
      };
    }
    // The conversation comes back with its history; the model and
    // permission mode are the ones chosen here, not the terminal's.
    return {
      tone: "muted" as const,
      content: <span>Continues with the model and permissions set here.</span>,
    };
  })();

  return (
    <div
      data-testid="composer-command-menu"
      data-menu="resume"
      className={cn(
        // Same anchor as the `+` menu: bottom-left, 8px above the
        // composer card. Wider, because rows carry two-line titles.
        "absolute bottom-full left-2 z-[60] mb-2 w-[520px] max-w-[calc(100%-1rem)]",
        "overflow-hidden rounded-[13px] border border-border",
        "bg-popover text-popover-foreground shadow-2xl",
        "rise-in",
      )}
      // Escape lives on the wrapper (outside cmdk's root) so it never
      // collides with cmdk's own arrow / Enter handling.
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onEscape();
        }
      }}
    >
      <CommandPrimitive
        shouldFilter={false}
        loop
        // Controlled on purpose: cmdk only reports highlight changes
        // (including its own first-item pick) for a controlled value.
        value={highlighted}
        onValueChange={setHighlighted}
        className="flex flex-col"
      >
        <div className="flex items-center gap-2.5 border-b border-border/60 px-3 py-2.5">
          <Search
            className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
            aria-hidden
          />
          <CommandPrimitive.Input
            ref={inputRef}
            value={query}
            onValueChange={onQueryChange}
            placeholder="Search terminal sessions…"
            data-testid="composer-command-search"
            className={cn(
              "flex-1 bg-transparent text-[13px] text-foreground",
              "outline-none placeholder:text-muted-foreground",
            )}
          />
          <span
            data-testid="resume-session-count"
            className="shrink-0 font-mono text-[9px] text-muted-foreground"
          >
            {grouping.total} on this machine
          </span>
        </div>
        <CommandPrimitive.List className="max-h-[340px] overflow-x-hidden overflow-y-auto p-1.5 outline-none">
          {grouping.folders.length === 0 && !loading && !error ? (
            <div className="px-3 py-6 text-center text-xs text-muted-foreground">
              {searching ? "No matches" : "Nothing to resume yet"}
            </div>
          ) : (
            <>
              {grouping.recent && (
                <CommandPrimitive.Group
                  heading="Recent"
                  data-testid="resume-recent"
                  className={cn(
                    "**:[[cmdk-group-heading]]:px-2",
                    "**:[[cmdk-group-heading]]:pt-1.5 **:[[cmdk-group-heading]]:pb-1",
                    "**:[[cmdk-group-heading]]:font-mono",
                    "**:[[cmdk-group-heading]]:text-[9.5px]",
                    "**:[[cmdk-group-heading]]:font-semibold",
                    "**:[[cmdk-group-heading]]:uppercase",
                    "**:[[cmdk-group-heading]]:tracking-[0.08em]",
                    "**:[[cmdk-group-heading]]:text-muted-foreground",
                  )}
                >
                  {grouping.recent.map((row) => renderRow(row, true))}
                </CommandPrimitive.Group>
              )}
              {grouping.recent && grouping.folders.length > 0 && (
                <div className="mx-2 my-1.5 h-px bg-border/60" />
              )}
              {grouping.folders.map((folder, index) => {
                const openNow = isExpanded(folder);
                return (
                  <Fragment key={folder.key}>
                    {renderFolderLine(folder, openNow)}
                    {openNow && folder.rows.map((row) => renderRow(row, false))}
                    {openNow && index < grouping.folders.length - 1 && (
                      <div className="mx-2 my-1.5 h-px bg-border/60" />
                    )}
                  </Fragment>
                );
              })}
            </>
          )}
        </CommandPrimitive.List>
        <div
          data-testid="slash-popup-footer"
          data-tone={footer.tone}
          className={cn(
            "flex items-center gap-2 border-t border-border/60 bg-background/60 px-3 py-2 text-[10.5px]",
            footer.tone === "error" ? "text-destructive" : "text-muted-foreground",
          )}
        >
          {footer.content}
        </div>
      </CommandPrimitive>
    </div>
  );
}

function EnterKey() {
  return (
    <kbd className="shrink-0 rounded border border-border/70 bg-background px-1 font-mono text-[9px] text-muted-foreground">
      ↵
    </kbd>
  );
}
