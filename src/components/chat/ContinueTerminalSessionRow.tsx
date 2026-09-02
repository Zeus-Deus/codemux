import { ChevronRight, Terminal } from "lucide-react";

import { agentDisplayName } from "@/lib/agent-chat/agent-display-name";
import { basename } from "@/lib/path";
import { relativeTime } from "@/lib/relative-time";
import { cn } from "@/lib/utils";
import type { AdoptableAgentSession } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

import { SCOPE_STRIP_INSET } from "./pickers/ThreadScopeRow";

/** A session younger than this is named on the landing with its own
 *  Continue button; anything older folds back into the quiet one-liner. */
export const FEATURED_MAX_AGE_MS = 24 * 60 * 60 * 1000;

/** What the draft is pointed at. A project draft only counts sessions
 *  from that project (its main checkout and its linked worktrees); a
 *  Home draft has no folder yet, so everything on the machine counts. */
export type LandingScope =
  | { kind: "home" }
  | { kind: "project"; projectRoot: string | null };

export type LandingSessionSummary =
  | { variant: "none" }
  | {
      variant: "featured";
      newest: AdoptableAgentSession;
      /** Sessions in scope besides `newest`. */
      more: number;
      inScope: number;
    }
  | { variant: "quiet"; newest: AdoptableAgentSession; inScope: number };

function normalizeDir(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.length > 0 ? trimmed : path;
}

/** The project a session belongs to: its repository root, or the folder
 *  itself when it ran outside any repository. */
function sessionProjectKey(session: AdoptableAgentSession): string {
  return normalizeDir(session.project_root ?? session.cwd);
}

function lastModifiedMs(session: AdoptableAgentSession): number {
  const ms = Date.parse(session.last_modified);
  return Number.isFinite(ms) ? ms : Number.NEGATIVE_INFINITY;
}

/**
 * Decide what the landing row says from the draft's discovery rows.
 *
 * The newest session IN SCOPE drives everything: younger than a day and
 * it is named outright ("featured"); older and the row is a quiet
 * count; no session in scope and the row is not rendered at all. A
 * project draft never borrows another project's session for the
 * featured slot — a fresh conversation elsewhere is not "in this
 * project", and the picker's collapsed groups are where it belongs.
 */
export function landingSessionSummary(
  sessions: readonly AdoptableAgentSession[],
  scope: LandingScope,
  now: Date = new Date(),
): LandingSessionSummary {
  let inScope: AdoptableAgentSession[];
  if (scope.kind === "home") {
    inScope = [...sessions];
  } else if (scope.projectRoot === null) {
    inScope = [];
  } else {
    const wanted = normalizeDir(scope.projectRoot);
    inScope = sessions.filter((s) => sessionProjectKey(s) === wanted);
  }
  if (inScope.length === 0) return { variant: "none" };
  inScope.sort((a, b) => lastModifiedMs(b) - lastModifiedMs(a));
  const newest = inScope[0]!;
  const ageMs = now.getTime() - lastModifiedMs(newest);
  if (Number.isFinite(ageMs) && ageMs < FEATURED_MAX_AGE_MS) {
    return {
      variant: "featured",
      newest,
      more: inScope.length - 1,
      inScope: inScope.length,
    };
  }
  return { variant: "quiet", newest, inScope: inScope.length };
}

function whenLabel(session: AdoptableAgentSession, now: Date): string {
  const ms = Date.parse(session.last_modified);
  return Number.isFinite(ms) ? relativeTime(new Date(ms), now) : "earlier";
}

/** Project name for a Home-draft row, where the session's project is
 *  the one thing the user cannot infer from context. */
function projectLabel(session: AdoptableAgentSession): string {
  return session.project_root ? basename(session.project_root) : "Home folder";
}

interface Props {
  /** Every session discovery found, across all projects. */
  sessions: readonly AdoptableAgentSession[];
  scope: LandingScope;
  /** Names the agent in the featured row's metadata. */
  provider: AgentChatProviderKind;
  disabled?: boolean;
  /** Opens the composer's `/resume` picker. */
  onOpenPicker: () => void;
  /** Resume this session directly — the same path a picker pick takes. */
  onContinue: (session: AdoptableAgentSession) => void;
  /** Injectable clock for tests. */
  now?: Date;
}

const ROW =
  "flex w-full items-center gap-2.5 rounded-xl border border-border/70 bg-muted/20 px-3 text-left text-xs";

/**
 * Second row under the draft's scope strip: the "continue a terminal
 * session" affordance, driven by what discovery found.
 *
 * Featured (a session in scope from the last day): names it, with a
 * Continue button that resumes it in one click and an "N more" link
 * into the picker. Quiet (older): a one-line count that opens the
 * picker. Nothing in scope: renders nothing, so an empty machine keeps
 * the bare headline + composer. The pick itself is handled by the
 * surface, exactly as a picker pick is.
 */
export function ContinueTerminalSessionRow({
  sessions,
  scope,
  provider,
  disabled,
  onOpenPicker,
  onContinue,
  now = new Date(),
}: Props) {
  const summary = landingSessionSummary(sessions, scope, now);
  if (summary.variant === "none") return null;

  const isHome = scope.kind === "home";
  const when = whenLabel(summary.newest, now);

  if (summary.variant === "quiet") {
    const where = isHome ? "on this machine" : "in this project";
    return (
      <div className={SCOPE_STRIP_INSET}>
        <button
          type="button"
          data-testid="draft-continue-terminal-session"
          data-variant="quiet"
          disabled={disabled}
          onClick={onOpenPicker}
          className={cn(
            ROW,
            "gap-2 py-1.5 text-muted-foreground outline-none transition-colors",
            "hover:bg-foreground/[0.04] hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring",
            "disabled:cursor-not-allowed disabled:opacity-50",
          )}
        >
          <Terminal className="size-3.5 shrink-0" aria-hidden />
          <span className="shrink-0 font-medium text-foreground">
            Continue a terminal session
          </span>
          <span className="min-w-0 flex-1 truncate">
            {summary.inScope} {where}, newest {when}
          </span>
          <kbd className="shrink-0 rounded border border-border/70 bg-background px-1 font-mono text-[10px] text-muted-foreground">
            /resume
          </kbd>
          <ChevronRight className="size-3 shrink-0 opacity-60" aria-hidden />
        </button>
      </div>
    );
  }

  const { newest, more } = summary;
  const checkout = newest.worktree_name
    ? `⑃ worktree ${newest.worktree_name}`
    : newest.git_branch;
  const meta = [
    isHome ? projectLabel(newest) : null,
    checkout,
    agentDisplayName(provider),
  ]
    .filter((part): part is string => !!part)
    .join(" · ");

  return (
    <div className={SCOPE_STRIP_INSET}>
      <div
        data-testid="draft-continue-terminal-session"
        data-variant="featured"
        className={cn(ROW, "py-2")}
      >
        <Terminal className="size-3.5 shrink-0 text-warning" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[11px] text-muted-foreground/80">
            You have a terminal session
            {isHome ? "" : " in this project"} from{" "}
            <span className="text-muted-foreground">{when}</span>
          </div>
          <div className="flex min-w-0 items-baseline gap-2">
            <span
              className="min-w-0 truncate text-foreground"
              data-testid="draft-continue-terminal-session-title"
            >
              {newest.title}
            </span>
            <span
              className="min-w-0 shrink-0 truncate font-mono text-[10px] text-muted-foreground/80"
              data-testid="draft-continue-terminal-session-meta"
            >
              {meta}
            </span>
          </div>
        </div>
        {more > 0 && (
          <button
            type="button"
            data-testid="draft-continue-terminal-session-more"
            disabled={disabled}
            onClick={onOpenPicker}
            className="flex shrink-0 items-center gap-0.5 rounded-md px-1.5 py-1 text-[11px] text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
          >
            {more} more
            <ChevronRight className="size-3 opacity-60" aria-hidden />
          </button>
        )}
        <button
          type="button"
          data-testid="draft-continue-terminal-session-continue"
          disabled={disabled}
          onClick={() => onContinue(newest)}
          className="shrink-0 rounded-md bg-foreground px-2.5 py-1 text-[11px] font-semibold text-background outline-none transition-opacity hover:opacity-90 focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
        >
          Continue
        </button>
      </div>
    </div>
  );
}
