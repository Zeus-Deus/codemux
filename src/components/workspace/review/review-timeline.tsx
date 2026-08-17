import { useMemo, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { activateTab } from "@/tauri/commands";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import type { AgentRunRecord } from "@/stores/pr-agent-runs-store";
import type { PrTimelineEvent } from "@/tauri/types";
import type { ChecksSummary, TimelineEntry, TimelineFilter } from "@/lib/pr-timeline";
import {
  groupDigits,
  plural,
  relativeAge,
  shortAge,
  tzBody,
  tzBodyLg,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
} from "./review-ui";

/**
 * The Timeline rail.
 *
 * The one screen here a host cannot render: agent runs sit in the
 * history next to the review comments that prompted them, on an ember
 * rail segment that says at a glance which entries are yours and local.
 *
 * Every entry has the same skeleton — a 17px gutter holding a dot and the
 * connecting line, then the content column — so an entry type this build
 * has never seen (`other`) still lands on the rail at the right place
 * rather than breaking it.
 */

/** Dot colours, one per meaning rather than one per event. */
type DotTone = "neutral" | "warn" | "green" | "ember" | "violet";

function Dot({ tone, spinning }: { tone: DotTone; spinning?: boolean }) {
  if (spinning) {
    return (
      <span
        aria-hidden
        data-testid="timeline-dot-spinner"
        className="size-[10px] flex-none animate-spin rounded-full border-[1.6px] border-status-working border-r-transparent"
      />
    );
  }
  return (
    <span
      aria-hidden
      data-tone={tone}
      className={cn(
        "size-[10px] flex-none rounded-full",
        tone === "neutral" && "border-[1.5px] border-border bg-card",
        tone === "warn" && "bg-status-working",
        tone === "green" && "bg-status-open",
        tone === "ember" && "bg-accent-ember",
        tone === "violet" && "bg-accent-violet",
      )}
    />
  );
}

function Row({
  tone,
  spinning,
  last,
  emberRail,
  children,
  testId,
}: {
  tone: DotTone;
  spinning?: boolean;
  /** The rail stops at the last entry — a line running into empty space
   *  reads as "still loading". */
  last: boolean;
  emberRail?: boolean;
  children: React.ReactNode;
  testId: string;
}) {
  return (
    <div className="flex gap-2.5" data-testid={testId}>
      <div className="flex w-[18px] flex-none flex-col items-center">
        <Dot tone={tone} spinning={spinning} />
        {/* The rail is what makes these one history rather than a stack
            of unrelated cards, so it is drawn at the border's own weight
            — at 40% it washed out and the entries read as floating. It
            stops at the last entry: a line running into empty space
            reads as "still loading". */}
        {!last && (
          <span
            aria-hidden
            data-testid="timeline-rail"
            data-rail-tone={emberRail ? "ember" : "neutral"}
            className={cn(
              "mt-1 w-[1.5px] flex-1 rounded-full",
              emberRail ? "bg-accent-ember/60" : "bg-border",
            )}
          />
        )}
      </div>
      <div className={cn("flex flex-1 flex-col gap-[3px]", !last && "pb-3.5")}>
        {children}
      </div>
    </div>
  );
}

const Actor = ({ name }: { name: string | null }) => (
  <span className="font-semibold text-foreground">{name ?? "Someone"}</span>
);

const Line = ({ children }: { children: React.ReactNode }) => (
  <span className={cn("text-foreground/80", tzBodyLg)}>{children}</span>
);

const Meta = ({ children }: { children: React.ReactNode }) => (
  <span className={cn("text-muted-foreground", tzMeta)}>{children}</span>
);

/** A review comment, quoted the way the thread list quotes it. */
function QuotedCard({ anchor, body }: { anchor?: string | null; body: string }) {
  if (!anchor && !body) return null;
  return (
    // Capped: on the full-width page a quote stretched to 1,400px, and
    // a line of prose that long stops being readable as a quote.
    <div className="flex max-w-[760px] flex-col gap-1 rounded-md bg-muted/40 px-3 py-2">
      {anchor && (
        <span className={cn("font-mono text-muted-foreground", tzEyebrow)}>{anchor}</span>
      )}
      {body && (
        <span className={cn("whitespace-pre-wrap leading-relaxed text-foreground/80", tzBody)}>
          {body}
        </span>
      )}
    </div>
  );
}

/** How a verdict reads as a verb, and which dot it earns. */
function verdictWording(verdict: string): { verb: string; tone: DotTone } {
  switch (verdict.toUpperCase()) {
    case "APPROVED":
      return { verb: "approved these changes", tone: "green" };
    case "CHANGES_REQUESTED":
      return { verb: "requested changes", tone: "warn" };
    case "DISMISSED":
      return { verb: "had a review dismissed", tone: "neutral" };
    default:
      return { verb: "reviewed this", tone: "neutral" };
  }
}

/** What the agent was sent to do, in the run card's voice. */
function runVerb(kind: AgentRunRecord["kind"]): string {
  if (kind === "review-thread") return "addressed this thread";
  if (kind === "failing-check") return "fixed a failing check";
  return "resolved conflicts";
}

function HostEntry({
  event,
  last,
  showChecksReran,
}: {
  event: PrTimelineEvent;
  last: boolean;
  /** Only the newest push says it — checks run against the latest head,
   *  so claiming it on an older push would be false. */
  showChecksReran: boolean;
}) {
  const age = relativeAge(event.created_at);
  const testId = `timeline-entry-${event.kind}`;

  switch (event.kind) {
    case "opened":
      return (
        <Row tone="neutral" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> opened this pull request
          </Line>
          <Meta>
            {[event.commits != null ? plural(event.commits, "commit") : null, age]
              .filter(Boolean)
              .join(" · ")}
          </Meta>
        </Row>
      );

    case "reviewed": {
      const { verb, tone } = verdictWording(event.verdict);
      return (
        <Row tone={tone} last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> {verb}
          </Line>
          <QuotedCard anchor={event.anchor} body={event.body} />
          {age && <Meta>{age}</Meta>}
        </Row>
      );
    }

    case "commented":
      return (
        <Row tone="neutral" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> commented
          </Line>
          <QuotedCard body={event.body} />
          {age && <Meta>{age}</Meta>}
        </Row>
      );

    case "committed":
      return (
        <Row tone="neutral" last={last} testId={testId}>
          <Line>
            Pushed{" "}
            <span className={cn("font-mono text-foreground", tzMetaNum)}>
              {event.sha.slice(0, 7)}
            </span>
            {event.message && <span className="text-muted-foreground"> {event.message}</span>}
          </Line>
          <Meta>{[age, showChecksReran ? "checks re-ran" : null].filter(Boolean).join(" · ")}</Meta>
        </Row>
      );

    case "head_ref_force_pushed":
      return (
        <Row tone="warn" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> force-pushed
            {event.sha && (
              <>
                {" to "}
                <span className={cn("font-mono text-foreground", tzMetaNum)}>
                  {event.sha.slice(0, 7)}
                </span>
              </>
            )}
          </Line>
          <Meta>{[age, showChecksReran ? "checks re-ran" : null].filter(Boolean).join(" · ")}</Meta>
        </Row>
      );

    case "merged":
      return (
        <Row tone="violet" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> merged this pull request
          </Line>
          {age && <Meta>{age}</Meta>}
        </Row>
      );

    case "closed":
    case "reopened":
      return (
        <Row tone={event.kind === "closed" ? "warn" : "neutral"} last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} />{" "}
            {event.kind === "closed" ? "closed this" : "reopened this"}
          </Line>
          {age && <Meta>{age}</Meta>}
        </Row>
      );

    case "review_requested":
      return (
        <Row tone="neutral" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> requested a review
            {event.reviewer && <> from <Actor name={event.reviewer} /></>}
          </Line>
          {age && <Meta>{age}</Meta>}
        </Row>
      );

    case "renamed":
      return (
        <Row tone="neutral" last={last} testId={testId}>
          <Line>
            <Actor name={event.actor} /> changed the title to “{event.to}”
          </Line>
          {age && <Meta>{age}</Meta>}
        </Row>
      );

    // An event this build has never seen. One line, its own label, never
    // dropped — a history with silent holes is worse than a plain row.
    case "other":
      return (
        <Row tone="neutral" last={last} testId="timeline-entry-other">
          <Line>
            <Actor name={event.actor} /> {event.label}
          </Line>
          {age && <Meta>{age}</Meta>}
        </Row>
      );
  }
}

function AgentEntry({
  run,
  last,
  onOpenThread,
}: {
  run: AgentRunRecord;
  last: boolean;
  onOpenThread: (run: AgentRunRecord) => void;
}) {
  const age = relativeAge(new Date(run.createdAt).toISOString());
  const stats =
    run.files != null
      ? [
          plural(run.files, "file"),
          run.additions != null ? `+${groupDigits(run.additions)}` : null,
          run.deletions != null ? `−${groupDigits(run.deletions)}` : null,
        ]
          .filter(Boolean)
          .join(" · ")
      : null;

  return (
    <Row tone="ember" emberRail last={last} testId="timeline-entry-agent">
      <Line>
        <span className="font-semibold text-accent-ember">Agent run</span> ·{" "}
        {runVerb(run.kind)}
      </Line>
      <div className="flex max-w-[760px] flex-col gap-1.5 rounded-md bg-accent-ember/[0.07] px-3 py-2">
        <span className={cn("leading-relaxed text-foreground/80", tzBody)}>
          {run.summary}
        </span>
        <div className="flex items-center gap-1.5">
          {/* Omitted rather than guessed — see `AgentRunRecord`. */}
          {stats && (
            <span className={cn("font-mono text-muted-foreground", tzEyebrow)}>{stats}</span>
          )}
          <span className="flex-1" />
          <button
            type="button"
            data-testid="timeline-open-thread"
            onClick={() => onOpenThread(run)}
            className={cn(
              "rounded border-0 bg-card px-2 py-1 text-foreground/90 outline-none transition-colors hover:bg-accent/50 focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
              tzMeta,
            )}
          >
            Open thread
          </button>
        </div>
      </div>
      <Meta>
        {[age, run.workspaceTitle ? `in ${run.workspaceTitle}` : null]
          .filter(Boolean)
          .join(" · ")}
      </Meta>
    </Row>
  );
}

function ChecksEntry({ checks }: { checks: ChecksSummary }) {
  return (
    <Row
      tone={checks.failed > 0 ? "warn" : "green"}
      spinning={checks.spinning}
      last
      testId="timeline-entry-checks"
    >
      <Line>{checks.sentence}</Line>
      <Meta>now</Meta>
    </Row>
  );
}

/**
 * Everything ▾ / Host only.
 *
 * "Host only" is not a de-emphasis of the agent runs — it removes them,
 * because the question it answers is "what would a teammate reading this
 * pull request on the web see?", and the honest answer is: not those.
 */
export function TimelineFilterPicker({
  value,
  onChange,
}: {
  value: TimelineFilter;
  onChange: (next: TimelineFilter) => void;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-testid="timeline-filter"
          className={cn(
            "border-0 bg-transparent py-2 text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
            tzMetaNum,
          )}
        >
          {value === "host" ? "Host only" : "Everything"}{" "}
          <span className={tzEyebrow}>▾</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-32">
        <DropdownMenuItem onSelect={() => onChange("everything")}>
          Everything
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onChange("host")}>Host only</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface ReviewTimelineProps {
  entries: TimelineEntry[];
  /** Never loaded once. Only this earns a skeleton (binding rule 2). */
  loading: boolean;
  /** Age of the newest good history when the poll is failing — the rail
   *  keeps rendering and says how old it is. */
  staleAgeMs: number | null;
}

export function ReviewTimeline({ entries, loading, staleAgeMs }: ReviewTimelineProps) {
  const [opening, setOpening] = useState<string | null>(null);

  /** The newest push, so only it claims that checks re-ran. */
  const newestPushId = useMemo(() => {
    for (let i = entries.length - 1; i >= 0; i--) {
      const entry = entries[i];
      if (
        entry.type === "host" &&
        (entry.event.kind === "committed" || entry.event.kind === "head_ref_force_pushed")
      ) {
        return entry.id;
      }
    }
    return null;
  }, [entries]);

  const hasChecksRow = entries.some((e) => e.type === "checks");

  /**
   * The ship-2 route, run backwards: the workspace the thread landed in,
   * then the tab it landed in when one was recorded.
   */
  const openThread = async (run: AgentRunRecord) => {
    if (opening) return;
    setOpening(run.id);
    try {
      await activateWorkspaceInteraction(run.workspaceId);
      if (run.threadTabId) await activateTab(run.workspaceId, run.threadTabId);
    } catch (err) {
      // The workspace may have been closed since the run — say so rather
      // than failing silently on a button that looks like it worked.
      toast.error(`Couldn't open that thread — ${String(err)}`);
    } finally {
      setOpening(null);
    }
  };

  if (loading && entries.length === 0) {
    return (
      <div
        className={cn("px-3.5 py-3 text-muted-foreground", tzBody)}
        data-testid="timeline-loading"
      >
        Loading history…
      </div>
    );
  }

  return (
    <div className="flex flex-col px-3.5 py-3" data-testid="review-timeline">
      {staleAgeMs != null && (
        <p className={cn("pb-2 text-muted-foreground", tzMeta)} data-testid="timeline-stale">
          Showing history from {shortAge(staleAgeMs)} ago
        </p>
      )}
      {entries.map((entry, index) => {
        const last = index === entries.length - 1;
        if (entry.type === "checks") return <ChecksEntry key={entry.id} checks={entry.checks} />;
        if (entry.type === "agent") {
          return (
            <AgentEntry
              key={entry.id}
              run={entry.run}
              last={last}
              onOpenThread={(run) => void openThread(run)}
            />
          );
        }
        return (
          <HostEntry
            key={entry.id}
            event={entry.event}
            last={last}
            // A push only claims "checks re-ran" when there are checks to
            // have re-run and it is the head they ran against.
            showChecksReran={hasChecksRow && entry.id === newestPushId}
          />
        );
      })}
    </div>
  );
}
