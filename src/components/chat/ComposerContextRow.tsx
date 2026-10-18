import { Folder, GitBranch, Home } from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { basename } from "@/lib/path";
import { cn } from "@/lib/utils";

import { nextFooterFit, type FooterFit } from "./Composer";
import {
  CONTEXT_ROW_HIDE_BRANCH,
  CONTEXT_ROW_HIDE_PROJECT,
} from "./pickers/footer-trigger";
import { SCOPE_STRIP, SCOPE_STRIP_INSET } from "./pickers/ThreadScopeRow";
import { WorkspaceStatusCluster } from "./WorkspaceStatusCluster";

/** The row steps down while the space between its two sides is below this. */
export const CONTEXT_ROW_MIN_GAP_PX = 12;

/** What the row gives up, one step at a time, while it is full. */
export interface ContextRowLadder {
  hideBranch: boolean;
  /** Effort drops its context window; access drops its label. */
  hideSecondary: boolean;
  /** Ahead / behind fold into the details popover. */
  hideGitCounts: boolean;
  hideProject: boolean;
  /** Effort keeps only its icon. */
  hideEffort: boolean;
}

export const CONTEXT_ROW_STEPS = 5;

export function contextRowLadder(level: number): ContextRowLadder {
  return {
    hideBranch: level >= 1,
    hideSecondary: level >= 2,
    hideGitCounts: level >= 3,
    hideProject: level >= 4,
    hideEffort: level >= 5,
  };
}

interface Props {
  projectRoot: string;
  isHomeWorkspace: boolean;
  branch: string | null;
  /** The composer's model, effort and access pickers. */
  settings: ReactNode;
  /** The pane's chat, for its pull-request watch chip. */
  threadId?: string | null;
}

/**
 * Context Row (design: `.design-import/Context Row.dc.html`): the strip
 * tucked under a live thread's composer. Project and branch on the left;
 * the composer's pickers, then the git/PR status cluster, on the right.
 *
 * Fits like the composer's controls row: it measures the gap between the
 * two sides and hides text in `contextRowLadder` order until the gap is
 * back above `CONTEXT_ROW_MIN_GAP_PX`, so it stays one line at any width.
 */
export function ComposerContextRow({
  projectRoot,
  isHomeWorkspace,
  branch,
  settings,
  threadId,
}: Props) {
  const rowRef = useRef<HTMLDivElement | null>(null);
  const gapRef = useRef<HTMLDivElement | null>(null);
  const [fit, setFit] = useState<FooterFit>({ key: "", level: 0, fitWidths: [] });
  // Bumped whenever the row's content changes (a label resolves, a PR
  // chip appears), so the fit restarts from step 0 and can win text back.
  const [contentVersion, bumpContent] = useReducer((n: number) => n + 1, 0);
  const committedFitRef = useRef(fit);

  const measure = useCallback((key?: string) => {
    const row = rowRef.current?.getBoundingClientRect().width ?? 0;
    const gap = gapRef.current?.getBoundingClientRect().width ?? 0;
    // No layout yet (or jsdom): keep everything.
    if (row <= 0) return;
    const measured = committedFitRef.current;
    setFit((prev) =>
      prev === measured
        ? nextFooterFit(
            prev,
            key ?? prev.key,
            row,
            gap,
            CONTEXT_ROW_MIN_GAP_PX,
            CONTEXT_ROW_STEPS,
          )
        : prev,
    );
  }, []);

  useEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const observers: Array<{ disconnect(): void }> = [];
    if (typeof ResizeObserver !== "undefined") {
      const resize = new ResizeObserver(() => measure());
      resize.observe(row);
      if (gapRef.current) resize.observe(gapRef.current);
      observers.push(resize);
    }
    if (typeof MutationObserver !== "undefined") {
      // Steps only toggle attributes on the row, so they never land here.
      const mutation = new MutationObserver(bumpContent);
      mutation.observe(row, { childList: true, subtree: true, characterData: true });
      observers.push(mutation);
    }
    return () => observers.forEach((observer) => observer.disconnect());
  }, [measure]);

  // Each step re-renders and re-measures before paint, so an overflowing
  // intermediate state never flashes.
  useLayoutEffect(() => {
    committedFitRef.current = fit;
    measure(`${projectRoot}|${branch ?? ""}|${contentVersion}`);
  }, [measure, fit, projectRoot, branch, contentVersion]);

  const ladder = contextRowLadder(fit.level);

  return (
    <div className={SCOPE_STRIP_INSET}>
      <div
        ref={rowRef}
        data-testid="composer-context-row"
        data-fit-level={fit.level}
        data-hide-branch={ladder.hideBranch || undefined}
        data-hide-secondary={ladder.hideSecondary || undefined}
        data-hide-git-counts={ladder.hideGitCounts || undefined}
        data-hide-project={ladder.hideProject || undefined}
        data-hide-effort={ladder.hideEffort || undefined}
        className={cn(SCOPE_STRIP, "group/ctx gap-0")}
        data-mobile-scope-strip
      >
        <div className="flex min-w-0 items-center gap-0.5 text-label font-medium text-muted-foreground">
          <span
            className={cn(
              "inline-flex h-6 min-w-0 items-center gap-1.5 rounded-md px-2",
              CONTEXT_ROW_HIDE_PROJECT,
            )}
            title={projectRoot}
          >
            {isHomeWorkspace ? (
              <Home className="size-3.5 shrink-0 text-status-remote" />
            ) : (
              <Folder className="size-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="max-w-[140px] truncate">
              {isHomeWorkspace ? "Home" : basename(projectRoot)}
            </span>
          </span>
          {branch && (
            <>
              <span
                className={cn(
                  "shrink-0 select-none text-muted-foreground/50",
                  CONTEXT_ROW_HIDE_BRANCH,
                )}
              >
                ·
              </span>
              <span
                className={cn(
                  "inline-flex h-6 min-w-0 items-center gap-1.5 rounded-md px-2 font-mono",
                  CONTEXT_ROW_HIDE_BRANCH,
                )}
                title={`Branch: ${branch}`}
              >
                <GitBranch className="size-3 shrink-0 text-muted-foreground" />
                <span className="max-w-[160px] truncate">{branch}</span>
              </span>
            </>
          )}
        </div>
        <div ref={gapRef} aria-hidden className="min-w-0 flex-1" />
        <div className="flex min-w-0 items-center gap-0.5">
          <div
            data-testid="context-row-settings"
            className="flex min-w-0 items-center gap-0.5"
          >
            {settings}
          </div>
          {/* The pipe belongs to the cluster: `empty:hidden` drops both
              when the cluster renders nothing. */}
          <div className="flex shrink-0 items-center before:mx-1 before:h-3.5 before:w-px before:bg-border/60 before:content-[''] empty:hidden">
            <WorkspaceStatusCluster threadId={threadId} />
          </div>
        </div>
      </div>
    </div>
  );
}
