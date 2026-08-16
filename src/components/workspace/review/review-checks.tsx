import { memo, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDown } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { getCheckLogExcerpt } from "@/tauri/commands";
import type { CheckInfo } from "@/tauri/types";
import { btnCard, checkState, type CheckState } from "./review-ui";

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
}

export function ReviewChecks({ checks, isLoading = false, cwd, prNumber }: Props) {
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
          <Skeleton className="h-1 w-full" />
          <Skeleton className="h-6 w-2/3" />
        </div>
      );
    }
    return (
      <p className="text-[11px] text-muted-foreground" data-testid="review-checks">
        No checks reported.
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1.5" data-testid="review-checks">
      <div className="flex items-center gap-2">
        <span className="flex flex-1 gap-0.5">
          {states.map((state, i) => (
            <span
              key={checks[i].name}
              title={checks[i].name}
              className={cn("h-1 flex-1 rounded-sm", SEGMENT_TONE[state])}
            />
          ))}
        </span>
        <span className="shrink-0 font-mono text-[10.5px] text-foreground/70">
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
}: {
  check: CheckInfo;
  state: CheckState;
  cwd: string;
  prNumber: number;
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

  const openLog = () => {
    if (!check.detail_url) return;
    openUrl(check.detail_url).catch((err) => toast.error(String(err)));
  };

  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 rounded-md px-2 py-1.5",
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
        <span className="min-w-0 flex-1 truncate text-[11.5px] text-foreground">
          {check.name}
        </span>
        {check.elapsed_time && (
          <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
            {check.elapsed_time}
          </span>
        )}
        <ChevronDown
          className={cn(
            "size-3 shrink-0 text-muted-foreground transition-transform",
            expanded && "rotate-180",
          )}
        />
      </button>

      {expanded && failing && (
        <div className="flex flex-col gap-1.5">
          {excerpt && (
            <pre className="overflow-hidden whitespace-pre-wrap break-all rounded bg-background/60 px-2 py-1.5 font-mono text-[9.5px] leading-relaxed text-destructive">
              {excerpt}
            </pre>
          )}
          {/* No Re-run button: `gh` has no per-check re-run, and a
              control that silently re-runs the whole workflow is not the
              one the label promises. No Fix-with-agent yet either — it
              arrives wired, or not at all. */}
          {check.detail_url && (
            <div className="flex items-center gap-1.5">
              <button type="button" className={btnCard} onClick={openLog}>
                Full log
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
