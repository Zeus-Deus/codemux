import "@fontsource/instrument-serif/latin-400-italic.css";
import { cn } from "@/lib/utils";

/** Where the hint sits relative to its picker trigger. */
export type ProjectPickerHintPlacement = "below" | "side";

// Each tail starts by the note and its head lands on the trigger: up-left
// under the headline's chevron ("below"), or left onto the "No project"
// button ("side"). Heads follow each curve's final tangent.
const PATHS: Record<
  ProjectPickerHintPlacement,
  { box: [number, number]; tail: string; head: string }
> = {
  below: {
    box: [120, 64],
    tail: "M 116 56 C 92 60, 68 56, 64 44 C 60 31, 78 27, 80 38 C 82 50, 52 52, 36 38 C 26 30, 16 20, 8 8",
    head: "M 8.5 17 L 8 8 L 16 12",
  },
  side: {
    box: [96, 44],
    tail: "M 92 32 C 78 40, 60 40, 58 28 C 56 16, 74 15, 74 26 C 74 37, 40 38, 6 22",
    head: "M 11 29.4 L 6 22 L 15 21.2",
  },
};

/**
 * A quiet, hand-drawn pointer at the new-chat project picker: a dotted
 * arrow and an italic note, so a first-time user learns the project in
 * the headline is a control. Decorative (aria-hidden); the trigger itself
 * carries the accessible name. The landing hides it once the picker has
 * been opened.
 */
export function ProjectPickerHint({
  placement,
}: {
  placement: ProjectPickerHintPlacement;
}) {
  const { box, tail, head } = PATHS[placement];
  const [w, h] = box;
  return (
    <span
      aria-hidden
      data-testid="project-picker-hint"
      className={cn(
        // Only where the pane has room beside the headline.
        "pointer-events-none absolute left-full hidden text-muted-foreground/70 @[46rem]:flex",
        "animate-in fade-in duration-250 delay-300 fill-mode-backwards motion-reduce:animate-none",
        placement === "below"
          ? "top-[85%] -ml-5 items-end"
          : "top-1/2 ml-1 -translate-y-1/2 items-center",
      )}
    >
      <svg
        width={w}
        height={h}
        viewBox={`0 0 ${w} ${h}`}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.7}
        strokeLinecap="round"
        strokeLinejoin="round"
        className="shrink-0"
      >
        {/* Dots, not dashes: a near-zero dash with round caps draws a dot. */}
        <path d={tail} strokeDasharray="0.01 5" />
        <path d={head} />
      </svg>
      <span
        // The note is CSS content so it stays out of the headline's text
        // (the hint sits inside the <h1>, beside its trigger).
        data-note="change project"
        // The sidebar's annotation voice (see sidebar-empty-state).
        style={{ fontFamily: "'Instrument Serif', Georgia, serif" }}
        className={cn(
          "ml-1 whitespace-nowrap text-[1.125rem] italic leading-none text-muted-foreground/80 after:content-[attr(data-note)]",
          placement === "below" ? "translate-y-1.5" : "translate-y-2",
        )}
      />
    </span>
  );
}
