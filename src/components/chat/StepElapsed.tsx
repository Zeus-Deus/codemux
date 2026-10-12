import type { ActivityStep } from "./transcript-slots";
import { stepElapsedLabel, stepStatus } from "./activity-steps";
import { TickingText } from "./TickingText";

/**
 * Ticking elapsed time for a running tool or thought, so a slow step can be
 * told apart from a hung one. Renders nothing once the step settles or when
 * it carries no start time; settled durations ride in the step's meta.
 */
export function StepElapsed({
  step,
  className,
  separated = false,
}: {
  step: ActivityStep;
  className?: string;
  /** Trail a ` ·` so the time reads apart from meta that follows it. */
  separated?: boolean;
}) {
  const startedAt = step.started_at;
  if (stepStatus(step) !== "running" || startedAt == null) return null;
  return (
    <TickingText
      active
      className={className}
      compute={(now) => {
        const label = stepElapsedLabel(startedAt, now);
        return separated && label ? `${label} ·` : label;
      }}
      testId="step-elapsed"
    />
  );
}
