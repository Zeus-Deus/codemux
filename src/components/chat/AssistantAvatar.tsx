import { Sparkle } from "lucide-react";

/**
 * The ember-tinted sparkle square shown once at the head of each
 * assistant turn (design D4). Sits in a 29px gutter column; subsequent
 * rows of the same turn render an empty gutter so their content stays
 * aligned under the first row.
 */
export function AssistantAvatar() {
  return (
    <span
      aria-hidden
      className="flex h-[29px] w-[29px] shrink-0 items-center justify-center rounded-[9px] bg-accent-ember/15 text-accent-ember"
    >
      <Sparkle className="h-[15px] w-[15px]" strokeWidth={1.4} />
    </span>
  );
}
