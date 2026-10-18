import type { AgentChatProviderKind } from "@/tauri/types";

/** A lookup that ignores inherited keys. Provider ids come from the ledger,
 *  and a plain index would answer "constructor" with a function. */
export function own<T>(table: Record<string, T>, key: string): T | undefined {
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : undefined;
}

const TOKEN_TIERS = [
  { scale: 1e12, suffix: "T" },
  { scale: 1e9, suffix: "B" },
  { scale: 1e6, suffix: "M" },
  { scale: 1e3, suffix: "K" },
] as const;

/** Three significant figures, trailing zeros dropped: 12.5M / 1.25M /
 *  804K / 8B / 640.
 *
 *  The B and T tiers are not hypothetical: provider history on a busy
 *  machine puts the 30-day figure in the billions. */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n)) return "0";
  const abs = Math.abs(n);
  for (const [index, tier] of TOKEN_TIERS.entries()) {
    if (abs < tier.scale) continue;
    const scaled = n / tier.scale;
    const digits = Math.abs(scaled) >= 100 ? 0 : Math.abs(scaled) >= 10 ? 1 : 2;
    const text = scaled.toFixed(digits);
    // 999.6K rounds to "1000K"; the next tier up reads it as "1M".
    if (Math.abs(Number(text)) >= 1000 && index > 0) {
      return `${trimZeros((n / TOKEN_TIERS[index - 1].scale).toFixed(2))}${TOKEN_TIERS[index - 1].suffix}`;
    }
    return `${trimZeros(text)}${tier.suffix}`;
  }
  const rounded = Math.round(n);
  return Math.abs(rounded) >= 1000 ? "1K" : String(rounded);
}

function trimZeros(text: string): string {
  return text.includes(".") ? text.replace(/\.?0+$/, "") : text;
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

/** Whole percents from 10% up, one decimal below that, and "<0.1%" for
 *  a share too small to show, so a real but tiny share never reads "0%". */
export function formatPercent(fraction: number): string {
  const percent = (Number.isFinite(fraction) ? fraction : 0) * 100;
  if (percent > 0 && percent < 0.1) return "<0.1%";
  if (percent >= 9.95 || percent === 0) return `${Math.round(percent)}%`;
  return `${trimZeros(percent.toFixed(1))}%`;
}

/** 1,234 sessions. */
export function formatCount(n: number): string {
  return Math.round(Number.isFinite(n) ? n : 0).toLocaleString("en-US");
}

/** Series colors, per the design-system token rules — no raw palette
 *  classes. Claude takes the brand accent, OpenCode the green status
 *  tone, Cursor violet, Grok sky and Codex a neutral foreground tint.
 *  Two neutrals side by side in a stacked column read as one block, so
 *  only one provider gets a grey. */
const SERIES_FILL: Record<string, string> = {
  claude: "bg-accent-ember",
  codex: "bg-foreground/45",
  cursor: "bg-accent-violet",
  grok: "bg-status-remote",
  opencode: "bg-status-open",
};

/** The same series tones as CSS colors, for the SVG chart. Codex's
 *  neutral tint is expressed as foreground at reduced opacity so it
 *  follows the palette the way the `bg-foreground/45` swatch does. */
const SERIES_COLOR: Record<string, { color: string; opacity: number }> = {
  claude: { color: "var(--accent-ember)", opacity: 1 },
  codex: { color: "var(--foreground)", opacity: 0.55 },
  cursor: { color: "var(--accent-violet)", opacity: 1 },
  grok: { color: "var(--status-remote)", opacity: 1 },
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
