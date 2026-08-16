import { memo, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDown } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { getCheckLogExcerpt } from "@/tauri/commands";
import type { CheckInfo } from "@/tauri/types";
import {
  btnCard,
  btnEmber,
  checkState,
  tzBody,
  tzBodyLg,
  tzLog,
  tzMeta,
  tzMetaNum,
  type CheckState,
} from "./review-ui";

/**
 * The spinner is its own memoized component on purpose: the checks query
 * refetches every 2.5s and hands this section a fresh array each tick.
 * Re-rendering the ring restarts its CSS animation, and a spinner that
 * visibly stutters every couple of seconds reads as the app struggling.
 * A primitive-string prop lets React skip the re-render entirely.
 */
const StateDot = memo(function StateDot({ state }: { state: CheckState }) {
  if (state === "running") {
    return (
      <span
        aria-hidden
        className="size-2.5 shrink-0 animate-spin rounded-full border-[1.6px] border-status-working border-r-transparent"
      />
    );
  }
  return (
    <span
      aria-hidden
      className={cn(
        "size-2.5 shrink-0 rounded-full",
        state === "pass" && "bg-status-open",
        state === "fail" && "bg-destructive",
        state === "neutral" && "bg-muted-foreground/50",
      )}
    />
  );
});

const SEGMENT_TONE: Record<CheckState, string> = {
  pass: "bg-status-open",
  fail: "bg-destructive",
  // A running check is a segment part-filled in amber — progress you can
  // read at a glance without a number.
  running: "bg-[linear-gradient(90deg,var(--color-status-working)_55%,var(--color-border)_55%)]",
  neutral: "bg-border",
};

interface Props {
  checks: CheckInfo[];
  isLoading?: boolean;
  /** Repo path + PR number, for the failing check's log excerpt. */
  cwd: string;
  prNumber: number;
  /**
   * Hands the failing check to an agent, with whatever excerpt this card
   * has already loaded. Absent ⇒ the button is not drawn: a control that
   * has nowhere to send the work is worse than no control.
   */
  onFixWithAgent?: (check: CheckInfo, logExcerpt: string) => Promise<unknown>;
  /** What the button will actually do, in this context. */
  handoffCaption?: string;
}

export function ReviewChecks({
  checks,
  isLoading = false,
  cwd,
  prNumber,
  onFixWithAgent,
  handoffCaption,
}: Props) {
  const { passed, states, notGreen } = useMemo(() => {
    const states = checks.map((c) => checkState(c.conclusion, c.status));
    return {
      states,
      passed: states.filter((s) => s === "pass").length,
      // Green checks are the ones you don't need to read. Only the
      // failing and running ones earn a row.
      notGreen: checks
        .map((check, i) => ({ check, state: states[i] }))
        .filter(({ state }) => state === "fail" || state === "running"),
    };
  }, [checks]);

  if (checks.length === 0) {
    if (isLoading) {
      return (
        <div className="flex flex-col gap-1.5" data-testid="review-checks">
          <Skeleton className="h-[3px] w-40" />
          <Skeleton className="h-6 w-2/3" />
        </div>
      );
    }
    return (
      <p className={cn("text-muted-foreground", tzBody)} data-testid="review-checks">
        No checks reported.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1.5" data-testid="review-checks">
      {/* The rail is a proportion, not a progress bar. One check
          stretched across the whole column read as "something is 100%
          done"; capping each segment keeps a two-check repo looking like
          a two-check repo, while a busy one still fills the width. */}
      <div className="flex items-center gap-2">
        {/* Capped as a whole as well as per segment, so the count stays
            beside the rail it counts instead of being flung to the far
            edge of a full-width detail column. */}
        <span className="flex max-w-[320px] flex-1 gap-0.5" data-testid="checks-rail">
          {states.map((state, i) => (
            <span
              key={checks[i].name}
              title={checks[i].name}
              data-testid="checks-rail-segment"
              className={cn(
                "h-[3px] max-w-[64px] flex-1 rounded-sm",
                SEGMENT_TONE[state],
              )}
            />
          ))}
        </span>
        <span className={cn("shrink-0 font-mono text-foreground/70", tzMetaNum)}>
          {passed} passed
        </span>
      </div>

      {notGreen.map(({ check, state }) => (
        <CheckRow
          key={check.name}
          check={check}
          state={state}
          cwd={cwd}
          prNumber={prNumber}
          onFixWithAgent={onFixWithAgent}
          handoffCaption={handoffCaption}
        />
      ))}
    </div>
  );
}

function CheckRow({
  check,
  state,
  cwd,
  prNumber,
  onFixWithAgent,
  handoffCaption,
}: {
  check: CheckInfo;
  state: CheckState;
  cwd: string;
  prNumber: number;
  onFixWithAgent?: (check: CheckInfo, logExcerpt: string) => Promise<unknown>;
  handoffCaption?: string;
}) {
  // A failing check is the thing you came to read, so it opens itself.
  const [expanded, setExpanded] = useState(state === "fail");
  const failing = state === "fail";

  // Only fetched once the card is actually open, and only for failures:
  // an excerpt is a nicety, not worth a subprocess per poll.
  const excerptQuery = useQuery({
    queryKey: ["pr", "check-log", prNumber, check.name] as const,
    queryFn: () => getCheckLogExcerpt(cwd, prNumber, check.name),
    enabled: failing && expanded,
    staleTime: 60_000,
    // The excerpt has no recovery path worth retrying for; the card
    // renders without it.
    retry: false,
  });
  const excerpt = excerptQuery.data?.trim() ?? "";
  const [handingOff, setHandingOff] = useState(false);

  const fixWithAgent = () => {
    if (!onFixWithAgent || handingOff) return;
    setHandingOff(true);
    // The excerpt may still be in flight; the handoff fetches its own
    // copy when this one is empty, so the button never has to wait for
    // a query it didn't start.
    onFixWithAgent(check, excerpt)
      .catch((err) => toast.error(String(err)))
      .finally(() => setHandingOff(false));
  };

  const openLog = () => {
    if (!check.detail_url) return;
    openUrl(check.detail_url).catch((err) => toast.error(String(err)));
  };

  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-md px-2.5 py-2",
        failing ? "bg-destructive/8" : "bg-muted/40",
      )}
      data-testid={`check-row-${check.name}`}
    >
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        className="flex items-center gap-1.5 text-left"
      >
        <StateDot state={state} />
        <span className={cn("min-w-0 flex-1 truncate text-foreground", tzBodyLg)}>
          {check.name}
        </span>
        {check.elapsed_time && (
          <span className={cn("shrink-0 font-mono text-muted-foreground", tzMeta)}>
            {check.elapsed_time}
          </span>
        )}
        <ChevronDown
          className={cn(
            "size-3.5 shrink-0 text-muted-foreground transition-transform",
            expanded && "rotate-180",
          )}
        />
      </button>

      {expanded && failing && (
        <div className="flex flex-col gap-1.5">
          {excerpt && (
            <pre
              className={cn(
                "overflow-hidden whitespace-pre-wrap break-all rounded bg-background/60 px-2.5 py-2 font-mono leading-[1.7] text-destructive",
                tzLog,
              )}
            >
              {excerpt}
            </pre>
          )}
          {/* No Re-run button: `gh` has no per-check re-run, and a
              control that silently re-runs the whole workflow is not the
              one the label promises. */}
          {(onFixWithAgent || check.detail_url) && (
            <div className="flex items-center gap-1.5">
              {onFixWithAgent && (
                <button
                  type="button"
                  // Same class string in both states: the label and the
                  // dot change, the box does not (binding rule 1).
                  className={btnEmber}
                  data-testid={`fix-with-agent-${check.name}`}
                  onClick={fixWithAgent}
                  disabled={handingOff}
                >
                  {handingOff ? (
                    <>
                      <span
                        aria-hidden
                        className="size-1.5 animate-spin rounded-full border-[1.5px] border-current border-r-transparent"
                      />
                      Starting agent
                    </>
                  ) : (
                    <>
                      <span
                        aria-hidden
                        className="size-1.5 rounded-full bg-current"
                      />
                      Fix with agent
                    </>
                  )}
                </button>
              )}
              {check.detail_url && (
                <button type="button" className={btnCard} onClick={openLog}>
                  Full log
                </button>
              )}
              {onFixWithAgent && handoffCaption && (
                <span className={cn("ml-auto truncate text-muted-foreground", tzMeta)}>
                  {handoffCaption}
                </span>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
