/**
 * The deck's 26px status foot: a 5px accent dot, the active pane's status
 * line, and — on the right — the focused thread's running token total.
 *
 * The token figure is the provider's own `total_processed_tokens` (the
 * lifetime count the composer's context meter already reads), formatted
 * with the shared compact formatter. When no thread is focused, or the
 * provider hasn't reported usage yet, the right side is simply absent
 * rather than showing a zero that would read as a real measurement.
 *
 * **No fill.** This row used to be `bg-card`, which drew a light slab across
 * the panel's bottom edge — the same defect the tab row had at the top (see
 * `pane-tab-strip.tsx`). The panel is one surface: its own `bg-background`
 * runs from the tab row's hairline to this one, and the two hairlines are
 * the only things dividing it. Unconditional, unlike the tab row's card fill
 * under legacy chrome — that exception exists because the tab row abuts a
 * real in-flow titlebar surface there, and this row abuts nothing.
 *
 * The dot and the text stay on their tokens (`accent-ember`,
 * `muted-foreground`), which are mixed against the foreground rather than
 * against the removed card fill, so contrast is unaffected.
 */
import { formatContextTokens } from "@/lib/agent-chat/context-usage";

export function PaneStatusFoot({
  status,
  tokens,
}: {
  status: string;
  tokens: number | null;
}) {
  return (
    <div
      data-testid="right-panel-status-foot"
      className="flex h-[26px] shrink-0 items-center gap-2 border-t border-border/60 bg-transparent px-3"
    >
      <span
        className="size-[5px] shrink-0 rounded-full bg-accent-ember"
        aria-hidden
      />
      <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground">
        {status}
      </span>
      {tokens != null && tokens > 0 && (
        <span
          data-testid="right-panel-token-total"
          className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground"
          title="Tokens processed in this thread"
        >
          Σ {formatContextTokens(tokens)} tok
        </span>
      )}
    </div>
  );
}
