import { ChevronRight, Terminal } from "lucide-react";

import { cn } from "@/lib/utils";

import { SCOPE_STRIP_INSET } from "./pickers/ThreadScopeRow";

interface Props {
  /** Conversations the agent's CLI left in the draft's current scope. */
  count: number;
  /** Where discovery looked, for the copy: `"checkout"` for a selected
   *  project (plus its worktrees), `"machine"` for a Home draft. */
  scope: "checkout" | "machine";
  disabled?: boolean;
  /** Opens the composer's `/resume` picker. */
  onOpen: () => void;
}

/**
 * Second row under the draft's scope strip: the "continue a terminal
 * session" affordance. Rendered only when discovery actually found
 * something, so an empty machine keeps the bare headline + composer.
 * The row is a pure trigger — the picker it opens is the same one
 * `/resume` uses, and the pick is handled by the surface.
 *
 * Neutral tokens only: the row sits inside the home landing, which
 * deliberately carries no accent colour.
 */
export function ContinueTerminalSessionRow({
  count,
  scope,
  disabled,
  onOpen,
}: Props) {
  const noun = count === 1 ? "conversation" : "conversations";
  const where =
    scope === "checkout" ? "in this checkout" : "on this machine";
  return (
    <div className={SCOPE_STRIP_INSET}>
      <button
        type="button"
        data-testid="draft-continue-terminal-session"
        disabled={disabled}
        onClick={onOpen}
        className={cn(
          "flex w-full items-center gap-2 rounded-xl border border-border/70 bg-muted/20 px-3 py-1.5 text-left text-xs text-muted-foreground outline-none transition-colors",
          "hover:bg-foreground/[0.04] hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring",
          "disabled:cursor-not-allowed disabled:opacity-50",
        )}
      >
        <Terminal className="size-3.5 shrink-0" aria-hidden />
        <span className="shrink-0 font-medium text-foreground">
          Continue a terminal session
        </span>
        <span className="min-w-0 flex-1 truncate">
          {count} {noun} started with the CLI {where}
        </span>
        <kbd className="shrink-0 rounded border border-border/70 bg-background px-1 font-mono text-[10px] text-muted-foreground">
          /resume
        </kbd>
        <ChevronRight className="size-3 shrink-0 opacity-60" aria-hidden />
      </button>
    </div>
  );
}
