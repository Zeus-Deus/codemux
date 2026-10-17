import type { CategoryCost, FlatModelUsage, UsageComposition } from "@/tauri/commands";
import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import { TermHint } from "./term-hint";
import { formatMoney, formatPercent, formatTokens } from "./usage-format";

export interface TokenTypeRow {
  key: "input" | "cache_read" | "cache_write" | "output" | "unsplit";
  /** Plain name, e.g. "Reused context". */
  label: string;
  /** The provider-docs name, e.g. "cache read", shown small next to the label. */
  technical?: string;
  /** One-sentence explanation shown on hover of the label. */
  hint: string;
  /** `null` when the kind has no tokens to count (unsplit cost). */
  tokens: number | null;
  cost: number;
  /** Optional muted note under the label, e.g. "73% of all input". */
  note?: string;
}

/** Below a cent, unsplit cost is rounding, not usage. */
const UNSPLIT_FLOOR_USD = 0.005;

function buildRows(
  tokens: { input: number; cacheRead: number; cacheWrite: number; output: number; reasoning: number },
  cost: CategoryCost,
  cacheReadShareOfInput: number,
): TokenTypeRow[] {
  const rows: TokenTypeRow[] = [
    {
      key: "input",
      label: "New input",
      technical: "uncached input",
      hint: "Prompt text the model read fresh, at the full input price.",
      tokens: tokens.input,
      cost: cost.input,
    },
    {
      key: "cache_read",
      label: "Reused context",
      technical: "cache read",
      hint: "Earlier conversation the provider had cached, so the model re-read it at about a tenth of the input price.",
      tokens: tokens.cacheRead,
      cost: cost.cache_read,
      note:
        tokens.cacheRead > 0 ? `${formatPercent(cacheReadShareOfInput)} of all input` : undefined,
    },
    {
      key: "cache_write",
      label: "Saved to cache",
      technical: "cache write",
      hint: "Context stored so later turns can reuse it. Usually priced a little above input.",
      tokens: tokens.cacheWrite,
      cost: cost.cache_write,
    },
    {
      key: "output",
      label: "Output",
      hint: "Text and code the model wrote, including hidden reasoning where the provider reports it.",
      tokens: tokens.output,
      cost: cost.output,
      // Only some providers split reasoning out; the rest say nothing.
      note: tokens.reasoning > 0 ? `incl. ${formatTokens(tokens.reasoning)} reasoning` : undefined,
    },
  ];
  if (cost.unsplit >= UNSPLIT_FLOOR_USD) {
    rows.push({
      key: "unsplit",
      label: "Not split by type",
      hint: "Cost the provider reported without a token breakdown.",
      tokens: null,
      cost: cost.unsplit,
    });
  }
  return rows;
}

/** The period's rows, from its composition and its cost by type. */
export function tokenTypeRows(
  composition: UsageComposition,
  categoryCost: CategoryCost,
): TokenTypeRow[] {
  return buildRows(
    {
      input: composition.input_tokens,
      cacheRead: composition.cache_read_tokens,
      cacheWrite: composition.cache_write_tokens,
      output: composition.output_tokens,
      reasoning: composition.reasoning_tokens,
    },
    categoryCost,
    composition.cache_read_share_of_input,
  );
}

/** One model's rows, from its own split (for the model dialog). */
export function modelTokenTypeRows(model: FlatModelUsage): TokenTypeRow[] {
  const input = model.input_tokens + model.cache_read_tokens + model.cache_write_tokens;
  return buildRows(
    {
      input: model.input_tokens,
      cacheRead: model.cache_read_tokens,
      cacheWrite: model.cache_write_tokens,
      output: model.output_tokens,
      reasoning: model.reasoning_tokens,
    },
    model.category_cost,
    input === 0 ? 0 : model.cache_read_tokens / input,
  );
}

/** A share of a total, or null when there is no total to share. */
function shareOf(value: number | null, total: number): number | null {
  return value === null || total <= 0 ? null : value / total;
}

function ShareTrack({
  share,
  side,
  emphasized,
}: {
  share: number | null;
  side: "tokens" | "cost";
  emphasized: boolean;
}) {
  return (
    <div aria-hidden className="relative h-2.5 min-w-0 overflow-hidden rounded-full bg-muted/40">
      {share !== null && share > 0 && (
        <div
          className={cn(
            "absolute inset-y-0 min-w-1 rounded-full transition-[width] duration-250 ease-out",
            // Both sides grow outward from the centre column.
            side === "tokens" ? "right-0" : "left-0",
            emphasized ? "bg-foreground" : "bg-foreground/45",
          )}
          style={{ width: `${Math.min(share, 1) * 100}%` }}
        />
      )}
    </div>
  );
}

function ShareValue({
  text,
  side,
  emphasized,
}: {
  text: string;
  side: "tokens" | "cost";
  emphasized: boolean;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "select-text whitespace-nowrap font-mono text-label tabular-nums",
        side === "tokens" ? "text-right" : "text-left",
        emphasized ? "text-foreground" : "text-muted-foreground",
      )}
    >
      {text}
    </span>
  );
}

/**
 * Where the tokens went beside where the money went, one row per kind of
 * token. Tokens grow left from the centre labels and cost grows right, on
 * the same 0–100% share-of-total scale, so a kind that is a sliver of the
 * tokens and most of the bill reads as exactly that.
 *
 * One five-column grid (value, track, label, track, value) with each row a
 * subgrid, so every track starts and ends at the same x whatever the width
 * of its figures.
 */
export function UsageTokenTypes({
  rows,
  emphasis,
  compact = false,
  className,
}: {
  rows: TokenTypeRow[];
  /** Which side matches the page's Cost/Tokens switch; that side draws at
   *  full ink, the other at 45%. */
  emphasis: "cost" | "tokens";
  /** Smaller rows and no header explanations, for the model dialog. */
  compact?: boolean;
  className?: string;
}) {
  const totalTokens = rows.reduce((sum, row) => sum + (row.tokens ?? 0), 0);
  const totalCost = rows.reduce((sum, row) => sum + row.cost, 0);
  const tokensEmphasized = emphasis === "tokens";
  const costEmphasized = emphasis === "cost";

  return (
    <div
      role="group"
      aria-label="Tokens and cost by type"
      className={cn(
        "grid items-center gap-x-3",
        compact
          ? "grid-cols-[auto_minmax(1.5rem,1fr)_minmax(7rem,9rem)_minmax(1.5rem,1fr)_auto]"
          : "grid-cols-[auto_minmax(2rem,1fr)_minmax(8rem,11rem)_minmax(2rem,1fr)_auto]",
        className,
      )}
    >
      <div aria-hidden className={cn("col-span-5 grid grid-cols-subgrid px-1", compact ? "pb-1" : "pb-2")}>
        <div className="col-span-2 flex flex-col items-end gap-0.5">
          <span className="flex items-baseline gap-2">
            <Eyebrow tone={tokensEmphasized ? "strong" : "muted"}>Tokens</Eyebrow>
            <span className="font-mono text-label tabular-nums text-foreground">
              {totalTokens > 0 ? formatTokens(totalTokens) : "—"}
            </span>
          </span>
          {!compact && (
            <span className="text-caption text-muted-foreground">share of all tokens</span>
          )}
        </div>
        <span />
        <div className="col-span-2 flex flex-col items-start gap-0.5">
          <span className="flex items-baseline gap-2">
            <Eyebrow tone={costEmphasized ? "strong" : "muted"}>Cost</Eyebrow>
            <span className="font-mono text-label tabular-nums text-foreground">
              {totalCost > 0 ? formatMoney(totalCost) : "—"}
            </span>
          </span>
          {!compact && (
            <span className="text-caption text-muted-foreground">share of all cost</span>
          )}
        </div>
      </div>

      {rows.map((row) => {
        const tokenShare = shareOf(row.tokens, totalTokens);
        const costShare = shareOf(row.cost, totalCost);
        const tokenText =
          row.tokens === null || tokenShare === null
            ? "—"
            : `${formatTokens(row.tokens)} · ${formatPercent(tokenShare)}`;
        const costText =
          costShare === null ? "—" : `${formatMoney(row.cost)} · ${formatPercent(costShare)}`;
        const spoken = [
          row.tokens === null || tokenShare === null
            ? "no tokens counted"
            : `${formatTokens(row.tokens)} tokens, ${formatPercent(tokenShare)} of tokens`,
          costShare === null
            ? "no cost recorded"
            : `${formatMoney(row.cost)}, ${formatPercent(costShare)} of cost`,
        ];
        return (
          <div
            key={row.key}
            data-token-type={row.key}
            className={cn(
              "col-span-5 grid grid-cols-subgrid items-center rounded-md px-1 transition-colors duration-150 hover:bg-accent/20",
              compact ? "py-1" : "py-1.5",
            )}
          >
            <span className="sr-only">{`${row.label}: ${spoken.join("; ")}.`}</span>
            <ShareValue text={tokenText} side="tokens" emphasized={tokensEmphasized} />
            <ShareTrack share={tokenShare} side="tokens" emphasized={tokensEmphasized} />
            <div className="flex min-w-0 flex-col items-center text-center">
              <span className="flex flex-wrap items-baseline justify-center gap-x-1.5">
                <TermHint hint={row.hint} className="text-body-sm text-foreground">
                  {row.label}
                </TermHint>
                {row.technical && (
                  <span className="font-mono text-caption text-muted-foreground">
                    {row.technical}
                  </span>
                )}
              </span>
              {row.note && <span className="text-caption text-muted-foreground">{row.note}</span>}
            </div>
            <ShareTrack share={costShare} side="cost" emphasized={costEmphasized} />
            <ShareValue text={costText} side="cost" emphasized={costEmphasized} />
          </div>
        );
      })}
    </div>
  );
}
