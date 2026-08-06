import { memo, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

import { CHAT_COLUMN } from "./chat-column";

/**
 * Docked "Monitoring in the background" bar.
 *
 * The calm sibling of {@link SubagentActivityBar}: same slot between the
 * transcript and the composer, same chat-column rails, opposite temperature.
 * The activity bar says "work is happening, wait"; this one says "the work is
 * done, but something is still being watched — and here is how to stop it".
 *
 * Renders only while the owning pane's status is `monitoring`, which is
 * reached two ways and does not care which: the Claude subagent tracker
 * settling a thread whose only live tasks are watch loops, or any agent at all
 * running `codemux monitor start`. Both arrive here as the same pane status.
 *
 * Nothing animates. A pulsing dot is this app's "look at me" vocabulary, and a
 * workspace babysitting CI overnight is precisely what should *not* be asking
 * for attention.
 *
 * The Stop button's pending state resolves on the status leaving `monitoring`
 * rather than on the command's promise: the command clears backend state and
 * the recomputed status arrives as a separate app-state emit, so "Stopping…"
 * ends when the thing the user asked for is visibly true. It also unwinds on a
 * thread switch, so a pending stop can never leak its spinner onto another
 * conversation.
 */
export const MonitoringBar = memo(function MonitoringBar({
  monitoring,
  reason,
  threadId,
  onStop,
}: {
  /** Whether the owning pane currently reports `monitoring`. */
  monitoring: boolean;
  /** Optional "what is being watched" text from `codemux monitor start
   *  --reason`. */
  reason?: string | null;
  /** Owning thread — resets the pending state across a thread switch. */
  threadId: string | null;
  onStop: () => void | Promise<void>;
}) {
  const [stopping, setStopping] = useState(false);
  const prevThreadRef = useRef(threadId);

  useEffect(() => {
    if (threadId !== prevThreadRef.current) {
      prevThreadRef.current = threadId;
      setStopping(false);
      return;
    }
    // The status leaving `monitoring` is the observable proof that the stop
    // landed — release the pending state on that, not on the promise.
    if (!monitoring) setStopping(false);
  }, [threadId, monitoring]);

  if (!monitoring) return null;

  const handleStop = () => {
    if (stopping) return;
    setStopping(true);
    // A failed stop must not strand the button in "Stopping…" forever; the
    // effect above handles the success path.
    void Promise.resolve(onStop()).catch((error) => {
      console.error("[MonitoringBar] stop failed", error);
      setStopping(false);
    });
  };

  return (
    <div className={CHAT_COLUMN}>
      <div
        data-testid="monitoring-bar"
        className={cn(
          "flex h-11 items-center gap-2.5 rounded-xl border px-3.5",
          "border-status-monitoring/25 bg-status-monitoring/[0.08]",
        )}
      >
        <span
          data-testid="monitoring-bar-dot"
          className="size-2 shrink-0 rounded-full bg-status-monitoring"
          aria-hidden
        />
        <span className="shrink-0 whitespace-nowrap text-[13px] font-bold text-foreground">
          Monitoring in the background
        </span>
        {reason ? (
          <>
            <span
              className="h-[3px] w-[3px] shrink-0 rounded-full bg-muted-foreground"
              aria-hidden
            />
            <span
              data-testid="monitoring-bar-reason"
              className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted-foreground"
            >
              {reason}
            </span>
          </>
        ) : (
          <span className="min-w-0 flex-1" />
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={stopping}
          onClick={handleStop}
          className="h-[30px] shrink-0 px-2.5 text-[11px] font-semibold"
        >
          {stopping ? "Stopping…" : "Stop"}
        </Button>
      </div>
    </div>
  );
});
