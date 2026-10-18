/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
  },
}));

vi.mock("@/tauri/commands", () => ({
  usageSummary: vi.fn(),
  usageExportCsv: vi.fn(),
  usageScanProviderHistory: vi.fn(),
  usageRefreshQuota: vi.fn(),
  usagePriceOverrides: vi.fn(),
  usageSetPriceOverride: vi.fn(),
}));

import {
  usageExportCsv,
  usagePriceOverrides,
  usageRefreshQuota,
  usageScanProviderHistory,
  usageSetPriceOverride,
  usageSummary,
} from "@/tauri/commands";
import type { FlatModelUsage, PlanUsageWindow, UsageSummary } from "@/tauri/commands";

import { TooltipProvider } from "@/components/ui/tooltip";
import {
  UsageSection,
  formatMoney,
  formatResetAt,
  formatTokens,
  meterTone,
  meterWindows,
  sortModels,
} from "./usage-section";
import { formatPercent } from "./usage-format";
import { filterUsageSummary } from "./usage-filter";

afterEach(() => cleanup());

beforeEach(() => {
  // Period/metric/view choices persist per device; tests start clean.
  localStorage.clear();
  vi.mocked(usageRefreshQuota).mockReset();
  vi.mocked(usageRefreshQuota).mockResolvedValue({
    quota: {},
    statuses: [],
    refreshed_at_ms: 0,
  });
  vi.mocked(usagePriceOverrides).mockReset();
  vi.mocked(usagePriceOverrides).mockResolvedValue({});
  vi.mocked(usageSetPriceOverride).mockReset();
});

function flatModel(overrides: Partial<FlatModelUsage>): FlatModelUsage {
  return {
    provider: "codex",
    model: "gpt-5-codex",
    tokens: 0,
    cost_usd: 0,
    priced: true,
    provider_reported: false,
    price_overridden: false,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    unpriced_tokens: 0,
    session_count: 1,
    category_cost: { input: 0, output: 0, cache_read: 0, cache_write: 0, unsplit: 0 },
    rates: null,
    cost_sources: { provider_usd: 0, table_usd: 0, override_usd: 0 },
    cache_savings: { savings_usd: 0, uncached_usd: 0, cached_usd: 0 },
    buckets: Array.from({ length: 7 }, () => ({ tokens: 0, cost_usd: 0 })),
    ...overrides,
  };
}

const DAY = 86_400_000;
const START = 1_800_000_000_000;

/** One value per day of the fixture's week. */
function week<T>(at: (day: number) => T): T[] {
  return Array.from({ length: 7 }, (_, day) => at(day));
}

/// A week of three providers that adds up the way real data does: every
/// model sits under its provider's lane, each lane is the sum of its
/// models, and the day buckets sum to the lanes and the totals.
///
/// Claude and OpenCode ran every day (Claude $9.40, OpenCode $0.92);
/// Codex ran once, on Aug 7, which makes that the busiest day ($29.56).
/// Totals: $91.48 and 11.1M tokens over 19 sessions.
function summary(overrides: Partial<UsageSummary> = {}): UsageSummary {
  return {
    period: "7d",
    start_ms: START,
    end_ms: START + 7 * DAY,
    buckets: week((i) => ({
      start_ms: START + i * DAY,
      label: String(i + 1),
      sub_label: `Aug ${i + 1}`,
      providers: {
        claude: { tokens: 1_000_000 + i * 1000, cost_usd: 9.4 },
        opencode: { tokens: 200_000, cost_usd: 0.92 },
        ...(i === 6 ? { codex: { tokens: 2_700_000, cost_usd: 19.24 } } : {}),
      },
    })),
    providers: [
      {
        provider: "claude",
        tokens: 7_021_000,
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cache_read_tokens: 5_221_000,
        cache_write_tokens: 300_000,
        cost_usd: 65.8,
        session_count: 12,
        models: [
          {
            model: "claude-opus-4-5",
            tokens: 4_900_000,
            cost_usd: 47.04,
            subagent_tokens: 0,
          },
          {
            model: "claude-haiku-4-5",
            tokens: 2_121_000,
            cost_usd: 18.76,
            subagent_tokens: 2_121_000,
          },
        ],
      },
      {
        provider: "codex",
        tokens: 2_700_000,
        input_tokens: 500_000,
        output_tokens: 200_000,
        cache_read_tokens: 1_900_000,
        cache_write_tokens: 100_000,
        cost_usd: 19.24,
        session_count: 4,
        models: [
          {
            model: "gpt-5-codex",
            tokens: 2_700_000,
            cost_usd: 19.24,
            subagent_tokens: 0,
          },
        ],
      },
      {
        provider: "opencode",
        tokens: 1_400_000,
        input_tokens: 400_000,
        output_tokens: 200_000,
        cache_read_tokens: 700_000,
        cache_write_tokens: 100_000,
        cost_usd: 6.44,
        session_count: 3,
        models: [
          {
            model: "anthropic/claude-sonnet-4-5",
            tokens: 1_060_000,
            cost_usd: 6.44,
            subagent_tokens: 0,
          },
          {
            model: "openrouter/kimi-k2",
            tokens: 340_000,
            cost_usd: 0,
            subagent_tokens: 0,
          },
        ],
      },
    ],
    totals: {
      estimated_cost_usd: 91.48,
      total_tokens: 11_121_000,
      cache_read_share: 0.7,
      session_count: 19,
    },
    composition: {
      processed_tokens: 11_121_000,
      cache_read_tokens: 7_821_000,
      // 7.821M reads of 10.221M observed input.
      cache_read_share_of_input: 0.7652,
      input_tokens: 1_900_000,
      cache_write_tokens: 500_000,
      output_tokens: 900_000,
      reasoning_tokens: 0,
      cache_savings_usd: 41.2,
      cache_savings_multiplier: 3.2,
    },
    // The models' own cost by type, summed.
    category_cost: {
      input: 5.63,
      output: 40.25,
      cache_read: 6.98,
      cache_write: 15.81,
      unsplit: 22.81,
    },
    confidence: {
      provider_reported_share: 0.07,
      table_priced_share: 0.93,
      override_priced_share: 0,
      // Kimi's 340K of 11.1M tokens.
      unpriced_token_share: 0.031,
      cache_savings_usd: 41.2,
    },
    models: [
      flatModel({
        provider: "claude",
        model: "claude-opus-4-5",
        tokens: 4_900_000,
        cost_usd: 47.04,
        input_tokens: 700_000,
        output_tokens: 350_000,
        cache_read_tokens: 3_650_000,
        cache_write_tokens: 200_000,
        session_count: 9,
        rates: { input: 5, output: 25, cache_read: 0.5, cache_write: 6.25 },
        category_cost: { input: 3.5, output: 26.25, cache_read: 5.48, cache_write: 11.81, unsplit: 0 },
        cost_sources: { provider_usd: 0, table_usd: 47.04, override_usd: 0 },
        buckets: week(() => ({ tokens: 700_000, cost_usd: 6.72 })),
      }),
      flatModel({
        provider: "claude",
        model: "claude-haiku-4-5",
        tokens: 2_121_000,
        cost_usd: 18.76,
        input_tokens: 300_000,
        output_tokens: 150_000,
        cache_read_tokens: 1_571_000,
        cache_write_tokens: 100_000,
        session_count: 3,
        rates: { input: 1, output: 5, cache_read: 0.1, cache_write: 1.25 },
        category_cost: { input: 1.5, output: 12, cache_read: 1.26, cache_write: 4, unsplit: 0 },
        cost_sources: { provider_usd: 0, table_usd: 18.76, override_usd: 0 },
        buckets: week((i) => ({ tokens: 300_000 + i * 1000, cost_usd: 2.68 })),
      }),
      flatModel({
        provider: "codex",
        model: "gpt-5-codex",
        tokens: 2_700_000,
        cost_usd: 19.24,
        input_tokens: 500_000,
        output_tokens: 200_000,
        cache_read_tokens: 1_900_000,
        cache_write_tokens: 100_000,
        session_count: 4,
        rates: { input: 1.25, output: 10, cache_read: 0.125, cache_write: 0 },
        category_cost: { input: 0.63, output: 2.0, cache_read: 0.24, cache_write: 0, unsplit: 16.37 },
        cost_sources: { provider_usd: 0, table_usd: 19.24, override_usd: 0 },
        buckets: week((i) => ({
          tokens: i === 6 ? 2_700_000 : 0,
          cost_usd: i === 6 ? 19.24 : 0,
        })),
      }),
      // OpenCode reports its own cost for the models it can price.
      flatModel({
        provider: "opencode",
        model: "anthropic/claude-sonnet-4-5",
        tokens: 1_060_000,
        cost_usd: 6.44,
        provider_reported: true,
        input_tokens: 160_000,
        output_tokens: 100_000,
        cache_read_tokens: 700_000,
        cache_write_tokens: 100_000,
        session_count: 2,
        category_cost: { input: 0, output: 0, cache_read: 0, cache_write: 0, unsplit: 6.44 },
        cost_sources: { provider_usd: 6.44, table_usd: 0, override_usd: 0 },
        buckets: week((i) => ({ tokens: i < 4 ? 115_000 : 200_000, cost_usd: 0.92 })),
      }),
      flatModel({
        provider: "opencode",
        model: "openrouter/kimi-k2",
        tokens: 340_000,
        cost_usd: 0,
        priced: false,
        unpriced_tokens: 340_000,
        input_tokens: 240_000,
        output_tokens: 100_000,
        buckets: week((i) => ({ tokens: i < 4 ? 85_000 : 0, cost_usd: 0 })),
      }),
    ],
    quota: {},
    synced_at_ms: START,
    ...overrides,
  };
}

/** The `<section>` a heading or eyebrow titles, so a figure the page
 *  repeats elsewhere (a lane, the readout) cannot satisfy an assertion
 *  meant for this one. */
function sectionOf(title: string): HTMLElement {
  return screen.getByText(title).closest("section") as HTMLElement;
}

const COST_CHART = "Estimated API value per day by provider";
const TOKENS_CHART = "Tokens per day by provider";

describe("number formatting", () => {
  it("abbreviates tokens to three significant figures", () => {
    expect(formatTokens(1_400_000)).toBe("1.4M");
    expect(formatTokens(1_254_000)).toBe("1.25M");
    expect(formatTokens(12_540_000)).toBe("12.5M");
    expect(formatTokens(82_000)).toBe("82K");
    expect(formatTokens(804_400)).toBe("804K");
    expect(formatTokens(640)).toBe("640");
    expect(formatTokens(0)).toBe("0");
  });

  /// Provider history on a busy machine puts the 30-day figure into
  /// the billions; without a B tier the hero read "7961.5M".
  it("carries billions and trillions tiers", () => {
    expect(formatTokens(7_961_500_000)).toBe("7.96B");
    expect(formatTokens(1_000_000_000)).toBe("1B");
    expect(formatTokens(1_234_567_890)).toBe("1.23B");
    expect(formatTokens(2_500_000_000_000)).toBe("2.5T");
    // A figure that rounds up to the next tier reads in that tier.
    expect(formatTokens(999_999_999)).toBe("1B");
    expect(formatTokens(999_600)).toBe("1M");
    expect(formatTokens(Number.POSITIVE_INFINITY)).toBe("0");
  });

  it("never rounds a real share down to 0%", () => {
    expect(formatPercent(0)).toBe("0%");
    expect(formatPercent(0.0004)).toBe("<0.1%");
    expect(formatPercent(0.043)).toBe("4.3%");
    expect(formatPercent(0.05)).toBe("5%");
    expect(formatPercent(0.374)).toBe("37%");
  });

  it("renders estimated money with cents", () => {
    expect(formatMoney(12.345)).toBe("$12.35");
    expect(formatMoney(0)).toBe("$0.00");
  });

  /// Five-figure estimates are normal once provider history is included
  /// in, and `$36999.88` misreads at a glance.
  it("groups thousands in large money figures", () => {
    expect(formatMoney(36_999.88)).toBe("$36,999.88");
    expect(formatMoney(1_234_567.891)).toBe("$1,234,567.89");
    // Small figures are untouched.
    expect(formatMoney(999.5)).toBe("$999.50");
  });
});

describe("UsageSection", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
  });

  it("renders the simple headline totals", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    expect(await screen.findByText("Estimated API value")).toBeInTheDocument();
    const headline = sectionOf("Estimated API value");
    expect(within(headline).getByText("$91.48")).toBeInTheDocument();
    expect(within(headline).getByText("across 19 sessions · last 7 days")).toBeInTheDocument();
    // The figure says what it is not: a bill.
    expect(within(headline).getByText(/^Not your bill: what this work would cost/)).toBeInTheDocument();
    expect(within(headline).getByRole("button", { name: "public API prices" })).toBeInTheDocument();
    // Each provider lane carries its share and the other metric.
    const claudeLane = screen.getByRole("list", { name: "Claude Code models" }).parentElement!;
    expect(within(claudeLane).getByText("72%")).toBeInTheDocument();
    expect(within(claudeLane).getByText("$65.80")).toBeInTheDocument();
    expect(within(claudeLane).getByText("7.02M tokens")).toBeInTheDocument();
    // Some tokens are unpriced, so the estimate says what it leaves out
    // and offers the fix in place.
    expect(
      within(headline).getByText(/^Excludes 3\.1% of tokens from models with no known price\./),
    ).toBeInTheDocument();
    await userEvent.click(within(headline).getByRole("button", { name: "Set prices" }));
    expect(await screen.findByRole("dialog", { name: "Model prices" })).toBeInTheDocument();
  });

  it("defaults to 7 days and refetches when the period changes", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));

    await userEvent.click(screen.getByRole("radio", { name: "30 days" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("30d"));

    await userEvent.click(screen.getByRole("radio", { name: "Past 24h" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("today"));
  });

  /// "Today" is a TRAILING 24-hour window, so the header must read the
  /// backend's own first/last bucket names rather than claiming
  /// "Today, 00:00 – now" — most of the chart is usually yesterday.
  it("names the today range from its real first and last buckets", async () => {
    const HOUR = 3_600_000;
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        period: "today",
        buckets: Array.from({ length: 24 }, (_, i) => ({
          start_ms: START + i * HOUR,
          label: `${(13 + i) % 24}:00`,
          sub_label:
            i < 11
              ? `Yesterday ${13 + i}:00`
              : `Today ${i - 11}:00`,
          providers: { claude: { tokens: 1_000, cost_usd: 0.1 } },
        })),
      }),
    );

    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(
        screen.getByText(/Yesterday 13:00 – Today 12:00/),
      ).toBeInTheDocument();
    });
  });

  it("shows a readout with per-provider figures for the hovered day", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    const chart = await screen.findByRole("group", { name: COST_CHART });
    expect(within(chart).queryByRole("tooltip")).not.toBeInTheDocument();

    // jsdom has no layout, so the chart lays out at its fallback width
    // with its rect at 0, and the far-left pointer lands on the first day.
    fireEvent.pointerMove(chart, { clientX: 0 });

    const readout = within(chart).getByRole("tooltip");
    expect(readout).toHaveTextContent("Aug 1");
    const row = (provider: string) =>
      readout.querySelector(`[data-readout-row="${provider}"]`) as HTMLElement;
    expect(within(row("claude")).getByText("Claude Code")).toBeInTheDocument();
    expect(within(row("claude")).getByText("$9.40")).toBeInTheDocument();
    expect(within(row("claude")).getByText("1M tok")).toBeInTheDocument();
    expect(within(row("opencode")).getByText("$0.92")).toBeInTheDocument();
    // Codex did not run that day, so it reads as idle rather than $0.00.
    expect(within(row("codex")).getByText("—")).toBeInTheDocument();
    expect(readout).toHaveTextContent("Total$10.32");
    // $10.32 against a $13.07 daily average.
    expect(readout).toHaveTextContent("vs period average−21%");
    expect(readout).toHaveTextContent("Top modelclaude-opus-4-5 · $6.72");
    // One pointer for the page: the lanes' day strips light the same day,
    // without a second readout of their own.
    const claudeStrip = screen.getByRole("img", { name: "Claude Code per day" });
    // The lanes' sparklines mark the same day with a point.
    expect(claudeStrip.querySelector('[data-day="0"]')).not.toBeNull();
    expect(within(claudeStrip).queryByRole("tooltip")).not.toBeInTheDocument();

    fireEvent.pointerLeave(chart);
    await waitFor(() => {
      expect(within(chart).queryByRole("tooltip")).not.toBeInTheDocument();
    });
  });

  it("switches the chart between cost and tokens", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    expect(await screen.findByRole("group", { name: COST_CHART })).toBeInTheDocument();
    expect(screen.getByText("Estimated API value")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("radio", { name: "Tokens" }));
    expect(await screen.findByRole("group", { name: TOKENS_CHART })).toBeInTheDocument();
    const headline = sectionOf("Tokens processed");
    expect(within(headline).getByText("11.1M")).toBeInTheDocument();
    expect(within(headline).getByText("across 19 sessions · last 7 days")).toBeInTheDocument();
    expect(within(headline).getByText(/^Every token the models read or wrote/)).toBeInTheDocument();
    // Under tokens the side figure is the cache share, not pricing.
    expect(within(headline).getByText("Reused from cache")).toBeInTheDocument();
    expect(within(headline).getByText("70%")).toBeInTheDocument();
    expect(screen.queryByText("How it's priced")).not.toBeInTheDocument();
  });

  /// Lane figures are API value, never a bill, so each says what its
  /// share is a share of.
  it("labels each lane's figure as a share of the API value", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    expect(await screen.findAllByText("of value")).toHaveLength(3);

    await userEvent.click(screen.getByRole("radio", { name: "Tokens" }));
    await waitFor(() => expect(screen.getAllByText("of tokens")).toHaveLength(3));
    expect(screen.queryByText("of value")).not.toBeInTheDocument();
  });

  it("recognizes Grok usage as a first-class provider", async () => {
    const base = summary();
    const baseProvider = base.providers[0]!;
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        providers: [
          {
            ...baseProvider,
            provider: "grok",
            models: [
              {
                model: "default",
                tokens: baseProvider.tokens,
                cost_usd: baseProvider.cost_usd,
                subagent_tokens: 0,
              },
            ],
          },
        ],
      }),
    );

    render(<UsageSection />, { wrapper: TooltipProvider });
    expect((await screen.findAllByText("Grok")).length).toBeGreaterThan(0);
    expect(screen.getAllByAltText("Grok")[0]).toHaveAttribute(
      "data-provider",
      "grok",
    );
  });

  it("nests each provider's models under its lane", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    // No expand step: the models are listed from the start, each under
    // the provider that ran it, largest first.
    const claude = await screen.findByRole("list", { name: "Claude Code models" });
    const modelButtons = (list: HTMLElement) =>
      within(list)
        .getAllByRole("button", { name: /^Open / })
        .map((button) => button.getAttribute("aria-label"));
    expect(modelButtons(claude)).toEqual([
      "Open claude-opus-4-5 details",
      "Open claude-haiku-4-5 details",
    ]);
    expect(modelButtons(screen.getByRole("list", { name: "Codex models" }))).toEqual([
      "Open gpt-5-codex details",
    ]);

    // Only the model with subagent tokens carries the subagent pill.
    const opus = within(claude).getByRole("button", { name: "Open claude-opus-4-5 details" });
    const haiku = within(claude).getByRole("button", { name: "Open claude-haiku-4-5 details" });
    expect(within(haiku).getByText("subagents")).toBeInTheDocument();
    expect(within(opus).queryByText("subagents")).not.toBeInTheDocument();
    // Shares are of the page's value: $47.04 and $18.76 of $91.48.
    expect(opus).toHaveTextContent("51%");
    expect(opus).toHaveTextContent("$47.04");
    expect(haiku).toHaveTextContent("21%");
    expect(haiku).toHaveTextContent("$18.76");
  });

  it("shows a quiet note instead of an empty chart when nothing ran", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        buckets: [],
        providers: [],
        totals: {
          estimated_cost_usd: 0,
          total_tokens: 0,
          cache_read_share: 0,
          session_count: 0,
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(
        screen.getByText("No agent activity in this period."),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText("Estimated API value")).not.toBeInTheDocument();
  });

  it("surfaces a load failure inline", async () => {
    vi.mocked(usageSummary).mockRejectedValue(new Error("db locked"));
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(screen.getByText(/Failed to load usage/)).toBeInTheDocument();
    });
    expect(screen.getByText(/db locked/)).toBeInTheDocument();
  });

  it("exports the CSV for the active period", async () => {
    // jsdom has no blob-URL plumbing; stub just enough for the download.
    const createObjectURL = vi.fn(() => "blob:usage");
    const revokeObjectURL = vi.fn();
    Object.assign(URL, { createObjectURL, revokeObjectURL });

    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalled());

    await userEvent.click(screen.getByRole("radio", { name: "30 days" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("30d"));

    await userEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    await waitFor(() => expect(usageExportCsv).toHaveBeenCalledWith("30d"));
    expect(createObjectURL).toHaveBeenCalled();
    expect(revokeObjectURL).toHaveBeenCalled();
  });
});

describe("plan quota meters", () => {
  // Own the mock rather than inheriting whatever the previous describe
  // block left behind — quota state differs per test here.
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
  });

  const win = (
    kind: PlanUsageWindow["kind"],
    used_pct: number,
    resets_at_ms: number | null = null,
  ): PlanUsageWindow => ({ kind, used_pct, resets_at_ms });

  it("uses status tones at the design's thresholds", () => {
    expect(meterTone(10)).toBe("bg-status-open");
    expect(meterTone(59.9)).toBe("bg-status-open");
    expect(meterTone(60)).toBe("bg-accent-ember");
    expect(meterTone(84.9)).toBe("bg-accent-ember");
    expect(meterTone(85)).toBe("bg-status-attention");
    expect(meterTone(100)).toBe("bg-status-attention");
  });

  it("shows 5h + overall weekly, not the per-model weeklies", () => {
    const picked = meterWindows([
      win("five_hour", 41),
      win("seven_day", 62),
      win("seven_day_opus", 88),
      win("seven_day_sonnet", 12),
    ]);
    expect(picked.map((w) => w.kind)).toEqual(["five_hour", "seven_day"]);
  });

  it("falls back to the highest per-model weekly when there is no overall one", () => {
    const picked = meterWindows([
      win("five_hour", 41),
      win("seven_day_opus", 88),
      win("seven_day_sonnet", 12),
    ]);
    expect(picked.map((w) => w.kind)).toEqual(["five_hour", "seven_day_opus"]);
  });

  it("formats reset time locally and omits it when unknown", () => {
    const at = new Date();
    at.setHours(16, 40, 0, 0);
    expect(formatResetAt(at.getTime())).toBe("resets 16:40");
    expect(formatResetAt(null)).toBe("");
    expect(formatResetAt(undefined)).toBe("");
  });

  it("renders meters and the plan label for a provider that reports quota", async () => {
    const at = new Date();
    at.setHours(16, 40, 0, 0);
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        quota: {
          claude: {
            windows: [
              win("five_hour", 41, at.getTime()),
              win("seven_day", 88),
            ],
            plan_label: "Max 20×",
            auth_mode: "subscription",
            received_at_ms: START,
          },
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    // Meters speak in what is LEFT, the same words as the Limits view.
    const meters = await screen.findByRole("button", {
      name: "Claude Code plan limits: 5h 59% left, week 12% left. Open Limits.",
    });
    expect(within(meters).getByText("5h")).toBeInTheDocument();
    expect(within(meters).getByText("59% left")).toBeInTheDocument();
    expect(within(meters).getByText("week")).toBeInTheDocument();
    expect(within(meters).getByText("12% left")).toBeInTheDocument();
    expect(screen.getByText("Max 20× · 12 sessions")).toBeInTheDocument();
    // The reset time is in the meter's own note.
    await userEvent.hover(within(meters).getByText("5h"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "5-hour window: 41% used, 59% left · resets 16:40.",
    );
    // The meters lead to the full Limits view.
    await userEvent.click(meters);
    expect(screen.getByRole("radio", { name: "Limits" })).toHaveAttribute("aria-checked", "true");
  });

  it("leaves a lane with no quota without meters", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByText("By tool and model");
    // No meters anywhere — an empty bar would imply a limit that does
    // not exist for OpenCode.
    expect(screen.queryByText("5h")).not.toBeInTheDocument();
    expect(screen.queryByText("week")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /plan limits/ })).not.toBeInTheDocument();
    expect(screen.getAllByText("No plan limits reported")).toHaveLength(3);
  });

  it("does not turn auth mode into a billing claim", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        quota: {
          claude: {
            windows: [],
            plan_label: null,
            auth_mode: "api_key",
            received_at_ms: START,
          },
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByText("By tool and model");
    // The lane still reads as API value with no plan, whatever the auth.
    expect(screen.getAllByText("of value")).toHaveLength(3);
    expect(screen.getByText("12 sessions")).toBeInTheDocument();
    expect(screen.queryByText(/billed to/)).not.toBeInTheDocument();
    expect(screen.queryByText(/api key/i)).not.toBeInTheDocument();
  });
});

describe("token types, over time and pricing", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
  });

  it("names each token type plainly, with its notes and what it means", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    const types = await screen.findByRole("group", { name: "Tokens and cost by type" });
    for (const label of [
      "New input",
      "Reused context",
      "Saved to cache",
      "Output",
      "Not split by type",
    ]) {
      expect(within(types).getByRole("button", { name: label })).toBeInTheDocument();
    }
    // Cache share is measured against observed INPUT, not all tokens.
    expect(
      within(types.querySelector('[data-token-type="cache_read"]') as HTMLElement).getByText(
        "77% of all input",
      ),
    ).toBeInTheDocument();

    const section = sectionOf("By token type");
    // 900K of 11.1M tokens, $40.25 of $91.48.
    expect(section).toHaveTextContent("Output was 8.1% of tokens but 44% of the value");
    expect(section).toHaveTextContent(
      "Caching saved about $41.20 — the same work would have cost 3.2× as much without it.",
    );
  });

  it("hides the reasoning note when no provider reported a split", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByRole("group", { name: "Tokens and cost by type" });
    expect(screen.queryByText(/reasoning/)).not.toBeInTheDocument();
  });

  it("shows the reasoning note once reasoning tokens exist", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        composition: { ...summary().composition, reasoning_tokens: 220_000 },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    const types = await screen.findByRole("group", { name: "Tokens and cost by type" });
    const output = types.querySelector('[data-token-type="output"]') as HTMLElement;
    expect(within(output).getByText("incl. 220K reasoning")).toBeInTheDocument();
  });

  it("marks an unpriced model in its provider's lane", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    const opencode = await screen.findByRole("list", { name: "OpenCode models" });
    const kimi = within(opencode).getByRole("button", { name: "Open openrouter/kimi-k2 details" });
    // Unpriced says so rather than reading $0.00, and its share is
    // expressed in TOKENS: 340K of 11.1M.
    expect(within(kimi).getByText("no price")).toBeInTheDocument();
    expect(within(kimi).getByText("No price")).toBeInTheDocument();
    expect(kimi).toHaveTextContent("3.1% tok");
    expect(kimi).not.toHaveTextContent("$0.00");
    // Under cost it ranks after the provider's priced model.
    const buttons = within(opencode).getAllByRole("button");
    expect(buttons[buttons.length - 1]).toBe(kimi);
  });

  it("switches Over time between the chart and a day table", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByRole("group", { name: COST_CHART });

    await userEvent.click(screen.getByRole("radio", { name: "Table" }));
    expect(screen.queryByRole("group", { name: COST_CHART })).not.toBeInTheDocument();
    // Day rows come from the same buckets the chart uses, newest first.
    // The first "Aug 7" is the busiest-day answer above the table.
    const overTime = sectionOf("Over time");
    expect(
      within(overTime)
        .getAllByText(/^Aug \d$/)
        .slice(1)
        .map((el) => el.textContent),
    ).toEqual(["Aug 7", "Aug 6", "Aug 5", "Aug 4", "Aug 3", "Aug 2", "Aug 1"]);
    expect(
      JSON.parse(localStorage.getItem("codemux.usage.preferences.v1")!).timeView,
    ).toBe("table");

    await userEvent.click(screen.getByRole("radio", { name: "Chart" }));
    expect(await screen.findByRole("group", { name: COST_CHART })).toBeInTheDocument();
  });

  it("renders every pricing-source row, zeros included", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        confidence: {
          provider_reported_share: 0,
          table_priced_share: 1,
          override_priced_share: 0,
          unpriced_token_share: 0,
          cache_savings_usd: 0,
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    const pricing = (await screen.findByText("How it's priced")).parentElement!;
    const share = (label: string) =>
      within(within(pricing).getByText(label).closest("li") as HTMLElement);
    // Zero rows still render so the block keeps a stable shape.
    expect(share("Reported by the provider").getByText("0%")).toBeInTheDocument();
    expect(share("Codemux price list").getByText("100%")).toBeInTheDocument();
    expect(share("Your prices").getByText("0%")).toBeInTheDocument();
    // Everything is priced, so there is nothing to call out.
    expect(screen.queryByText(/no known price/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Set prices" })).not.toBeInTheDocument();
  });

  it("offers a 90-day period that refetches", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));
    await userEvent.click(screen.getByRole("radio", { name: "90 days" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("90d"));
  });

  it("refetches when the refresh button is pressed", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledTimes(2));
    // Same period, not a period change.
    expect(vi.mocked(usageSummary).mock.calls.every(([p]) => p === "7d")).toBe(true);
  });
});

describe("provider history", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageScanProviderHistory).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
    vi.mocked(usageScanProviderHistory).mockResolvedValue({
      files_scanned: 34,
      sessions_found: 12,
      rows_updated: 5,
      reimported: false,
    });
  });

  it("renders the footer note with all provider histories", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(
        screen.getByText("Includes 19 sessions from Claude Code, Codex and OpenCode on this machine"),
      ).toBeInTheDocument();
    });
    expect(screen.getByText("~/.claude/projects")).toBeInTheDocument();
    expect(screen.getByText("~/.codex/sessions")).toBeInTheDocument();
  });

  it("scans provider history on open with no launcher-specific toggle", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageScanProviderHistory).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    expect(screen.queryByText("excluded")).not.toBeInTheDocument();
  });

  it("reports the period's total sessions, not the last scan's changed ones", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        totals: {
          estimated_cost_usd: 72.24,
          total_tokens: 8_421_000,
          cache_read_share: 0.7,
          session_count: 430,
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(
        screen.getByText(/^Includes 430 sessions from/),
      ).toBeInTheDocument();
    });
    // Emphatically not the scan report's figure.
    expect(screen.queryByText(/Includes 12 /)).not.toBeInTheDocument();
  });

  it("singularizes a lone provider session", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        totals: {
          estimated_cost_usd: 1,
          total_tokens: 100,
          cache_read_share: 0,
          session_count: 1,
        },
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => {
      expect(
        screen.getByText(/^Includes 1 session from/),
      ).toBeInTheDocument();
    });
  });

  it("exports the CSV for the visible period", async () => {
    Object.assign(URL, {
      createObjectURL: vi.fn(() => "blob:x"),
      revokeObjectURL: vi.fn(),
    });
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalled());
    await userEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    await waitFor(() => expect(usageExportCsv).toHaveBeenCalledWith("7d"));
  });

  /// A refresh must re-scan too, or new provider records stay invisible.
  it("re-scans on refresh", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageScanProviderHistory).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    await waitFor(() => expect(usageScanProviderHistory).toHaveBeenCalledTimes(2));
  });
});

describe("limits view", () => {
  const HOUR = 3_600_000;

  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageScanProviderHistory).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
  });

  it("reads plan limits directly on open", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageRefreshQuota).toHaveBeenCalledTimes(1));
  });

  it("lists every window with quota left and a reset countdown", async () => {
    const now = Date.now();
    vi.mocked(usageRefreshQuota).mockResolvedValue({
      quota: {
        claude: {
          windows: [
            { kind: "seven_day", used_pct: 72, resets_at_ms: now + 50 * HOUR, window_mins: 10_080 },
            { kind: "five_hour", used_pct: 10, resets_at_ms: now + 2 * HOUR + 60_000, window_mins: 300 },
            { kind: "other", used_pct: 0, resets_at_ms: null, label: "Weekly · Fable" },
          ],
          plan_label: "Claude Max",
          received_at_ms: now,
        },
      },
      statuses: [
        { provider: "claude", outcome: "ok" },
        { provider: "codex", outcome: "failed", message: "timed out" },
      ],
      refreshed_at_ms: now,
    });
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "Limits" }));

    expect(await screen.findByText("Claude Max")).toBeInTheDocument();
    // Five-hour first, whatever order the provider reported.
    const labels = screen.getAllByText(/^(5-hour|Weekly|Weekly · Fable)$/).map((el) => el.textContent);
    expect(labels).toEqual(["5-hour", "Weekly", "Weekly · Fable"]);
    expect(screen.getByText("90%")).toBeInTheDocument();
    expect(screen.getByText("28%")).toBeInTheDocument();
    // Render time trails the fixture's clock by a few ms, so match the hour.
    expect(screen.getByText(/^resets in 2h [01]m$/)).toBeInTheDocument();
    // A failed read is said out loud, not shown as an empty provider.
    expect(screen.getByText("Could not read limits: timed out")).toBeInTheDocument();
    // The period does not apply to limits.
    expect(screen.getByRole("radio", { name: "7 days" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Export CSV" })).toBeDisabled();
  });

  it("reports a failed read without blaming one provider", async () => {
    vi.mocked(usageRefreshQuota).mockRejectedValue("IPC unavailable");
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "Limits" }));
    expect(
      await screen.findByText("Could not read plan limits: IPC unavailable"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Claude Code")).not.toBeInTheDocument();
  });

  it("explains an empty result instead of rendering nothing", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "Limits" }));
    expect(
      await screen.findByText(/No provider on this machine reports plan limits/),
    ).toBeInTheDocument();
  });

  it("re-reads limits from the refresh button without rescanning history", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "Limits" }));
    await waitFor(() => expect(usageRefreshQuota).toHaveBeenCalledTimes(1));
    const scans = vi.mocked(usageScanProviderHistory).mock.calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Refresh limits" }));
    await waitFor(() => expect(usageRefreshQuota).toHaveBeenCalledTimes(2));
    expect(vi.mocked(usageScanProviderHistory).mock.calls.length).toBe(scans);
  });

  it("feeds direct readings into the lane meters", async () => {
    const now = Date.now();
    vi.mocked(usageRefreshQuota).mockResolvedValue({
      quota: {
        claude: {
          windows: [{ kind: "five_hour", used_pct: 33, resets_at_ms: null }],
          plan_label: "Claude Max",
          received_at_ms: now,
        },
      },
      statuses: [{ provider: "claude", outcome: "ok" }],
      refreshed_at_ms: now,
    });
    render(<UsageSection />, { wrapper: TooltipProvider });
    expect(await screen.findByText("67% left")).toBeInTheDocument();
    expect(screen.getByText("Claude Max · 12 sessions")).toBeInTheDocument();
  });

  it("remembers the chosen view and period across visits", async () => {
    const first = render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "30 days" }));
    await userEvent.click(screen.getByRole("radio", { name: "Limits" }));
    first.unmount();

    render(<UsageSection />, { wrapper: TooltipProvider });
    expect(await screen.findByRole("radio", { name: "Limits" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: "30 days" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });
});

describe("model details and prices", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageScanProviderHistory).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
  });

  it("splits the period's cost and tokens by type", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    const types = await screen.findByRole("group", { name: "Tokens and cost by type" });
    // Each row reads its tokens and its cost against the period's totals:
    // 1.9M of 11.1M tokens and $5.63 of $91.48 for new input.
    expect(
      within(types).getByText("New input: 1.9M tokens, 17% of tokens; $5.63, 6.2% of cost."),
    ).toHaveClass("sr-only");
    expect(
      within(types).getByText("Output: 900K tokens, 8.1% of tokens; $40.25, 44% of cost."),
    ).toBeInTheDocument();
    // Provider-reported cost with no split has no tokens to count.
    expect(
      within(types).getByText("Not split by type: no tokens counted; $22.81, 25% of cost."),
    ).toBeInTheDocument();
  });

  it("opens a model's detail with its own stats", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("button", { name: "Open gpt-5-codex details" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("gpt-5-codex");
    expect(dialog).toHaveTextContent("$19.24");
    // 1.9M reads of 2.5M observed input.
    expect(dialog).toHaveTextContent("Input from cache76%");
    // $19.24 over 2.7M tokens.
    expect(dialog).toHaveTextContent("Avg per 1M tokens$7.13");
    expect(dialog).toHaveTextContent("Codemux's list-price table");
  });

  it("prices an unpriced model and refetches with the new price", async () => {
    vi.mocked(usageSetPriceOverride).mockResolvedValue({
      "openrouter/kimi-k2": { input: 0.6, output: 2.5, cache_read: 0, cache_write: 0 },
    });
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(
      await screen.findByRole("button", { name: "Open openrouter/kimi-k2 details" }),
    );
    expect(await screen.findByText("340K tokens have no known price")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Set price" }));

    await userEvent.type(screen.getByLabelText("Input"), "0.6");
    await userEvent.type(screen.getByLabelText("Output"), "2.5");
    const fetches = vi.mocked(usageSummary).mock.calls.length;
    await userEvent.click(screen.getByRole("button", { name: "Save price" }));

    await waitFor(() =>
      expect(usageSetPriceOverride).toHaveBeenCalledWith("openrouter/kimi-k2", {
        input: 0.6,
        output: 2.5,
        cache_read: 0,
        cache_write: 0,
      }),
    );
    await waitFor(() =>
      expect(vi.mocked(usageSummary).mock.calls.length).toBeGreaterThan(fetches),
    );
  });

  it("opens a fresh price editor for a model named like an Object key", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        models: [
          flatModel({
            provider: "opencode",
            model: "constructor",
            tokens: 1_000,
            priced: false,
            unpriced_tokens: 1_000,
          }),
        ],
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("button", { name: "Open constructor details" }));
    await userEvent.click(await screen.findByRole("button", { name: "Set price" }));
    expect(await screen.findByText("Set model price")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Use default price" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("Input")).toHaveValue("");
  });

  it("lists custom prices and removes one", async () => {
    vi.mocked(usagePriceOverrides).mockResolvedValue({
      "openrouter/kimi-k2": { input: 0.6, output: 2.5, cache_read: 0, cache_write: 0 },
    });
    vi.mocked(usageSetPriceOverride).mockResolvedValue({});
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("button", { name: "Model prices" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("openrouter/kimi-k2");
    expect(dialog).toHaveTextContent("$2.50");
    await userEvent.click(
      screen.getByRole("button", { name: "Remove price for openrouter/kimi-k2" }),
    );
    await waitFor(() =>
      expect(usageSetPriceOverride).toHaveBeenCalledWith("openrouter/kimi-k2", null),
    );
    expect(await screen.findByText(/No custom prices yet/)).toBeInTheDocument();
  });

  it("breaks each day down by provider", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(await screen.findByRole("radio", { name: "Table" }));
    // The table under its "Day" header, apart from the answer line above it.
    const table = within(sectionOf("Over time")).getByText("Day").parentElement!.parentElement!;
    // Headers name what each column measures, not just "Total".
    expect(within(table).getByText("Total value")).toBeInTheDocument();
    expect(within(table).getByText("Tokens")).toBeInTheDocument();
    expect(
      within(sectionOf("Over time")).getByText(/Each tool's estimated API value per day/),
    ).toBeInTheDocument();
    // Claude spent $9.40 every day in the fixture, OpenCode $0.92; Codex
    // ran only on Aug 7 and is dashed on the other days.
    expect(within(table).getAllByText("$9.40")).toHaveLength(7);
    expect(within(table).getAllByText("$0.92")).toHaveLength(7);
    expect(within(table).getAllByText("$19.24")).toHaveLength(1);
    expect(within(table).getAllByText("—")).toHaveLength(6);
    expect(within(table).getAllByText("$10.32")).toHaveLength(6);
    expect(within(table).getAllByText("$29.56")).toHaveLength(1);
  });
});

describe("provider filter", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
  });

  it("rebuilds every provider-scoped figure from the remaining models", () => {
    const base = summary({
      models: [
        flatModel({
          provider: "claude",
          model: "claude-opus-4-5",
          tokens: 7_021_000,
          cost_usd: 65.8,
          input_tokens: 1_000_000,
          output_tokens: 500_000,
          cache_read_tokens: 5_221_000,
          cache_write_tokens: 300_000,
          cost_sources: { provider_usd: 0, table_usd: 65.8, override_usd: 0 },
          cache_savings: { savings_usd: 40, uncached_usd: 105.8, cached_usd: 65.8 },
          category_cost: { input: 5, output: 37.5, cache_read: 7.8, cache_write: 15.5, unsplit: 0 },
        }),
        flatModel({
          provider: "opencode",
          model: "openrouter/kimi-k2",
          tokens: 1_400_000,
          cost_usd: 6.44,
          provider_reported: true,
          cost_sources: { provider_usd: 6.44, table_usd: 0, override_usd: 0 },
          category_cost: { input: 0, output: 0, cache_read: 0, cache_write: 0, unsplit: 6.44 },
        }),
      ],
    });
    const onlyClaude = filterUsageSummary(base, new Set(["opencode", "codex"]));
    expect(onlyClaude.providers.map((p) => p.provider)).toEqual(["claude"]);
    expect(onlyClaude.models.map((m) => m.model)).toEqual(["claude-opus-4-5"]);
    expect(onlyClaude.totals.estimated_cost_usd).toBe(65.8);
    expect(onlyClaude.totals.session_count).toBe(12);
    expect(onlyClaude.confidence.table_priced_share).toBe(1);
    expect(onlyClaude.confidence.provider_reported_share).toBe(0);
    expect(onlyClaude.composition.cache_savings_usd).toBe(40);
    expect(onlyClaude.composition.cache_savings_multiplier).toBeCloseTo(105.8 / 65.8);
    expect(onlyClaude.category_cost.unsplit).toBe(0);
    expect(Object.keys(onlyClaude.buckets[0]!.providers)).toEqual(["claude"]);
    // Nothing hidden that the summary contains: the summary itself.
    expect(filterUsageSummary(base, new Set(["cursor"]))).toBe(base);
  });

  it("hides a provider from the page and remembers it", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(
      await screen.findByRole("button", { name: "Filter providers: All providers" }),
    );
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: /OpenCode/ }));
    await userEvent.keyboard("{Escape}");

    // Claude's $65.80 plus Codex's $19.24.
    await waitFor(() => {
      expect(within(sectionOf("Estimated API value")).getByText("$85.04")).toBeInTheDocument();
    });
    expect(screen.queryByRole("list", { name: "OpenCode models" })).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Filter providers: 2 of 3 providers" }),
    ).toBeInTheDocument();
    expect(
      JSON.parse(localStorage.getItem("codemux.usage.preferences.v1")!).hiddenProviders,
    ).toEqual(["opencode"]);
  });

  it("will not hide the last visible provider", async () => {
    localStorage.setItem(
      "codemux.usage.preferences.v1",
      JSON.stringify({ hiddenProviders: ["opencode", "codex"] }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await userEvent.click(
      await screen.findByRole("button", { name: "Filter providers: Claude Code" }),
    );
    expect(await screen.findByRole("menuitemcheckbox", { name: /Claude Code/ })).toHaveAttribute(
      "data-disabled",
    );
  });
});

describe("breakdown ordering and shortcuts", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
  });

  it("ranks models by the active metric, unpriced last under cost", () => {
    const models = [
      flatModel({ model: "cheap-but-big", tokens: 9_000, cost_usd: 1 }),
      flatModel({ model: "unpriced", tokens: 20_000, cost_usd: 0, priced: false }),
      flatModel({ model: "pricey", tokens: 1_000, cost_usd: 5 }),
    ];
    expect(sortModels(models, "cost").map((m) => m.model)).toEqual([
      "pricey",
      "cheap-but-big",
      "unpriced",
    ]);
    expect(sortModels(models, "tokens").map((m) => m.model)).toEqual([
      "unpriced",
      "cheap-but-big",
      "pricey",
    ]);
  });

  it("switches view and period from the keyboard only when asked to", async () => {
    const { unmount } = render(<UsageSection shortcuts />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));

    await userEvent.keyboard("3");
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("30d"));
    await userEvent.keyboard("t");
    expect(await screen.findByRole("group", { name: TOKENS_CHART })).toBeInTheDocument();
    await userEvent.keyboard("l");
    expect(screen.getByRole("radio", { name: "Limits" })).toHaveAttribute("aria-checked", "true");
    unmount();

    // Settings renders the same section without page shortcuts.
    vi.mocked(usageSummary).mockClear();
    render(<UsageSection />, { wrapper: TooltipProvider });
    await waitFor(() => expect(usageSummary).toHaveBeenCalled());
    await userEvent.keyboard("c");
    expect(screen.getByRole("radio", { name: "Limits" })).toHaveAttribute("aria-checked", "true");
  });

  it("keeps the previous period's figures up, dimmed, until the new one lands", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByText("Estimated API value");
    let resolve: (value: UsageSummary) => void = () => {};
    vi.mocked(usageSummary).mockReturnValue(new Promise((r) => (resolve = r)));

    await userEvent.click(screen.getByRole("radio", { name: "30 days" }));
    const content = screen.getByText("Estimated API value").closest("[aria-busy]");
    expect(content).toHaveAttribute("aria-busy", "true");

    resolve(summary({ period: "30d" }));
    await waitFor(() =>
      expect(screen.getByText("Estimated API value").closest("[aria-busy]")).toBeNull(),
    );
  });
});

describe("over time and the model tree", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
  });

  /// Pointing at a model shows its part of every day on the chart, so a
  /// spike can be traced to the model behind it.
  it("traces a model's part of each day on the chart while its row is pointed at", async () => {
    render(<UsageSection />, { wrapper: TooltipProvider });
    const chart = await screen.findByRole("group", { name: COST_CHART });
    expect(chart.querySelector("[data-overlay]")).toBeNull();

    // Pointing at a model draws its own line over the providers'.
    const codex = screen.getByRole("button", { name: "Open gpt-5-codex details" });
    await userEvent.hover(codex);
    await waitFor(() => expect(chart.querySelectorAll("[data-overlay]")).toHaveLength(1));
    expect(sectionOf("Over time")).toHaveTextContent("Dashed line: gpt-5-codex on its own");
    await userEvent.unhover(codex);
    await waitFor(() => expect(chart.querySelector("[data-overlay]")).toBeNull());

    // Opus ran every day; keyboard focus works the same as the pointer.
    const opus = screen.getByRole("button", { name: "Open claude-opus-4-5 details" });
    act(() => opus.focus());
    await waitFor(() => expect(chart.querySelectorAll("[data-overlay]")).toHaveLength(1));
    act(() => opus.blur());
    await waitFor(() => expect(chart.querySelector("[data-overlay]")).toBeNull());
  });

  it("names the busiest day and its share of the period", async () => {
    // Move Codex's one run to midweek, so the busiest day is not simply
    // the latest one.
    const base = summary();
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        buckets: base.buckets.map((bucket, i) => ({
          ...bucket,
          providers: {
            claude: bucket.providers.claude!,
            opencode: bucket.providers.opencode!,
            ...(i === 2 ? { codex: { tokens: 2_700_000, cost_usd: 19.24 } } : {}),
          },
        })),
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    await screen.findByRole("group", { name: COST_CHART });
    // $9.40 + $0.92 + $19.24 of $91.48.
    expect(sectionOf("Over time")).toHaveTextContent(
      "Busiest day: Aug 3 · $29.56, 32% of the period",
    );

    await userEvent.click(screen.getByRole("radio", { name: "Tokens" }));
    // 1.002M + 200K + 2.7M of 11.1M tokens.
    await waitFor(() =>
      expect(sectionOf("Over time")).toHaveTextContent(
        "Busiest day: Aug 3 · 3.9M, 35% of the period",
      ),
    );
  });

  it("lists a provider's top three models and reveals the rest on request", async () => {
    // Three more Claude models, carved out of Haiku so the lane still
    // adds up: 120K tokens and $0.94 between them.
    const base = summary();
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        models: [
          ...base.models.map((m) =>
            m.model === "claude-haiku-4-5"
              ? { ...m, tokens: m.tokens - 120_000, cost_usd: 17.82 }
              : m,
          ),
          flatModel({ provider: "claude", model: "claude-sonnet-4-5", tokens: 90_000, cost_usd: 0.6 }),
          flatModel({ provider: "claude", model: "claude-opus-4-1", tokens: 20_000, cost_usd: 0.3 }),
          flatModel({ provider: "claude", model: "claude-3-5-haiku", tokens: 10_000, cost_usd: 0.04 }),
        ],
      }),
    );
    render(<UsageSection />, { wrapper: TooltipProvider });
    const claude = await screen.findByRole("list", { name: "Claude Code models" });
    const shown = () =>
      within(claude)
        .getAllByRole("button", { name: /^Open / })
        .map((button) => button.getAttribute("aria-label"));
    expect(shown()).toEqual([
      "Open claude-opus-4-5 details",
      "Open claude-haiku-4-5 details",
      "Open claude-sonnet-4-5 details",
    ]);

    await userEvent.click(within(claude).getByRole("button", { name: "Show 2 more" }));
    expect(shown()).toEqual([
      "Open claude-opus-4-5 details",
      "Open claude-haiku-4-5 details",
      "Open claude-sonnet-4-5 details",
      "Open claude-opus-4-1 details",
      "Open claude-3-5-haiku details",
    ]);

    await userEvent.click(within(claude).getByRole("button", { name: "Show fewer" }));
    expect(shown()).toHaveLength(3);
    // A provider with three models or fewer has nothing to reveal.
    expect(
      within(screen.getByRole("list", { name: "Codex models" })).queryByRole("button", {
        name: /^Show/,
      }),
    ).not.toBeInTheDocument();
  });
});
