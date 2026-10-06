import type { AgentChatProviderKind } from "@/tauri/types";

/** A lookup that ignores inherited keys. Provider ids come from the ledger,
 *  and a plain index would answer "constructor" with a function. */
function own<T>(table: Record<string, T>, key: string): T | undefined {
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : undefined;
}

/** 8.0B / 1.4M / 82K / 640 — the design's `toks`.
 *
 *  The B tier is not hypothetical: provider history on a busy machine
 *  puts the 30-day figure in the billions, and without it the hero read
 *  "7961.5M" — technically correct, unreadable, and visibly wider than
 *  the stat next to it. */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n)) return "0";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}K`;
  return String(Math.round(n));
}

/** Thousands separators keep large API-equivalent estimates readable. */
const MONEY = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
/** $12.34 / $36,999.88 — an API/list-price equivalent, not an invoice. */
export function formatMoney(n: number): string {
  return `$${MONEY.format(Number.isFinite(n) ? n : 0)}`;
}

export function formatPercent(fraction: number): string {
  return `${Math.round((Number.isFinite(fraction) ? fraction : 0) * 100)}%`;
}

/** Series colors, per the design-system token rules — no raw palette
 *  classes. Claude takes the brand accent, OpenCode the green status
 *  tone, and Codex a neutral foreground tint (Codemux has no third
 *  brand hue, and inventing one would imply a status meaning). */
const SERIES_FILL: Record<string, string> = {
  claude: "bg-accent-ember",
  codex: "bg-foreground/45",
  cursor: "bg-accent-violet",
  grok: "bg-foreground/70",
  opencode: "bg-status-open",
};

/** The same series tones as CSS colors, for the SVG chart. Codex's
 *  neutral tint is expressed as foreground at reduced opacity so it
 *  follows the palette the way the `bg-foreground/45` swatch does. */
const SERIES_COLOR: Record<string, { color: string; opacity: number }> = {
  claude: { color: "var(--accent-ember)", opacity: 1 },
  codex: { color: "var(--foreground)", opacity: 0.55 },
  cursor: { color: "var(--accent-violet)", opacity: 1 },
  grok: { color: "var(--foreground)", opacity: 0.8 },
  opencode: { color: "var(--status-open)", opacity: 1 },
};

const UNKNOWN_COLOR = { color: "var(--muted-foreground)", opacity: 0.5 };

export function seriesColor(provider: string): { color: string; opacity: number } {
  return own(SERIES_COLOR, provider) ?? UNKNOWN_COLOR;
}

const SERIES_LABEL: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  opencode: "OpenCode",
};

/** Fallback for a provider id the frontend does not know — the ledger
 *  outlives the provider list, so a row from a since-removed adapter
 *  must still render rather than crash. */
const UNKNOWN_FILL = "bg-muted-foreground/40";

export function seriesFill(provider: string): string {
  return own(SERIES_FILL, provider) ?? UNKNOWN_FILL;
}

export function seriesLabel(provider: string): string {
  return own(SERIES_LABEL, provider) ?? provider;
}

export function isKnownProvider(provider: string): provider is AgentChatProviderKind {
  return own(SERIES_LABEL, provider) !== undefined;
}
