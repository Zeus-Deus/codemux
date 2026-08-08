/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
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
  usageScanCliLogs: vi.fn(),
}));

import {
  usageExportCsv,
  usageScanCliLogs,
  usageSummary,
} from "@/tauri/commands";
import type { PlanUsageWindow, UsageSummary } from "@/tauri/commands";

import {
  UsageSection,
  formatMoney,
  formatMoney0,
  formatResetAt,
  formatTokens,
  meterTone,
  meterWindows,
} from "./usage-section";

afterEach(() => cleanup());

const DAY = 86_400_000;
const START = 1_800_000_000_000;

function summary(overrides: Partial<UsageSummary> = {}): UsageSummary {
  return {
    period: "7d",
    start_ms: START,
    end_ms: START + 7 * DAY,
    buckets: Array.from({ length: 7 }, (_, i) => ({
      start_ms: START + i * DAY,
      label: String(i + 1),
      sub_label: `Aug ${i + 1}`,
      providers: {
        claude: { tokens: 1_000_000 + i * 1000, cost_usd: 9.4 },
        opencode: { tokens: 200_000, cost_usd: 0.92 },
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
        billing: "plan_covered",
        models: [
          {
            model: "claude-opus-4-5",
            tokens: 5_000_000,
            cost_usd: 47.0,
            subagent_tokens: 0,
          },
          {
            model: "claude-haiku-4-5",
            tokens: 2_021_000,
            cost_usd: 18.8,
            subagent_tokens: 2_021_000,
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
        billing: "metered",
        models: [
          {
            model: "openrouter/kimi-k2",
            tokens: 1_400_000,
            cost_usd: 6.44,
            subagent_tokens: 0,
          },
        ],
      },
    ],
    totals: {
      metered_cost_usd: 6.44,
      plan_covered_cost_usd: 65.8,
      total_tokens: 8_421_000,
      cache_read_share: 0.7,
      session_count: 15,
      workspace_count: 4,
      cli_session_count: 6,
    },
    composition: {
      processed_tokens: 8_421_000,
      cache_read_tokens: 5_921_000,
      cache_read_share_of_input: 0.82,
      input_tokens: 1_400_000,
      cache_write_tokens: 400_000,
      output_tokens: 700_000,
      reasoning_tokens: 0,
      cache_savings_usd: 41.2,
      cache_savings_multiplier: 3.2,
    },
    confidence: {
      provider_reported_share: 0.09,
      table_priced_share: 0.91,
      unpriced_token_share: 0.04,
      cache_savings_usd: 41.2,
    },
    models: [
      {
        provider: "codex",
        model: "gpt-5-codex",
        tokens: 2_700_000,
        cost_usd: 19.24,
        priced: true,
        provider_reported: false,
      },
      {
        provider: "opencode",
        model: "openrouter/kimi-k2",
        tokens: 340_000,
        cost_usd: 0,
        priced: false,
        provider_reported: false,
      },
    ],
    quota: {},
    synced_at_ms: START,
    ...overrides,
  };
}

describe("number formatting", () => {
  it("abbreviates tokens the way the design does", () => {
    expect(formatTokens(1_400_000)).toBe("1.4M");
    expect(formatTokens(82_000)).toBe("82K");
    expect(formatTokens(640)).toBe("640");
    expect(formatTokens(0)).toBe("0");
  });

  /// Folding in this machine's CLI history puts the 30-day figure into
  /// the billions; without a B tier the hero read "7961.5M".
  it("carries a billions tier", () => {
    expect(formatTokens(7_961_500_000)).toBe("8.0B");
    expect(formatTokens(1_000_000_000)).toBe("1.0B");
    expect(formatTokens(1_234_567_890)).toBe("1.2B");
    // …and the boundary still belongs to M.
    expect(formatTokens(999_999_999)).toBe("1000.0M");
    expect(formatTokens(Number.POSITIVE_INFINITY)).toBe("0");
  });

  it("renders owed money with cents and plan value in whole dollars", () => {
    expect(formatMoney(12.345)).toBe("$12.35");
    expect(formatMoney(0)).toBe("$0.00");
    // Plan-covered value is an estimate — cents would imply precision
    // it does not have.
    expect(formatMoney0(65.8)).toBe("$66");
  });

  /// Five-figure dollar amounts are normal once CLI history is folded
  /// in, and `$36999.88` misreads at a glance.
  it("groups thousands in large money figures", () => {
    expect(formatMoney(36_999.88)).toBe("$36,999.88");
    expect(formatMoney(1_234_567.891)).toBe("$1,234,567.89");
    expect(formatMoney0(36_999.88)).toBe("$37,000");
    expect(formatMoney0(1_234_567)).toBe("$1,234,567");
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

  it("renders the four hero stats from the totals", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Metered")).toBeInTheDocument();
    });
    // $6.44 is the metered hero AND the OpenCode lane/legend figure —
    // the same number by design, so assert presence, not uniqueness.
    expect(screen.getAllByText("$6.44").length).toBeGreaterThan(0);
    expect(screen.getByText("Plan-covered")).toBeInTheDocument();
    expect(screen.getByText("$66")).toBeInTheDocument();
    // 8.4M is both the hero Tokens stat and the composition strip's
    // Processed figure — the same number by design.
    expect(screen.getAllByText("8.4M").length).toBeGreaterThan(0);
    expect(screen.getByText("15")).toBeInTheDocument();
    expect(screen.getByText("70% served from cache")).toBeInTheDocument();
    expect(screen.getByText("across 4 workspaces")).toBeInTheDocument();
  });

  it("defaults to 7 days and refetches when the period changes", async () => {
    render(<UsageSection />);
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));

    await userEvent.click(screen.getByRole("radio", { name: "30 days" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("30d"));

    await userEvent.click(screen.getByRole("radio", { name: "Today" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("today"));
  });

  it("switches the readout between cost and tokens", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Total for period")).toBeInTheDocument();
    });
    // Cost mode distinguishes the two billing kinds in the breakdown.
    expect(screen.getByText(/metered/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("radio", { name: "Tokens" }));
    await waitFor(() => {
      expect(screen.getByText("Tokens for period")).toBeInTheDocument();
    });
    expect(screen.getAllByText("input + output + cache read").length).toBeGreaterThan(0);
  });

  it("labels each lane with its billing kind", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Metered · your API keys")).toBeInTheDocument();
    });
    expect(screen.getByText("Covered by plan")).toBeInTheDocument();
    expect(screen.getByText("billed to your keys")).toBeInTheDocument();
    expect(screen.getByText("covered by plan · at list")).toBeInTheDocument();
  });

  it("reveals per-model rows only when a lane is expanded", async () => {
    render(<UsageSection />);
    // "Claude Code" appears in both the legend and the lane header.
    await waitFor(() => {
      expect(screen.getAllByText("Claude Code").length).toBeGreaterThan(0);
    });
    expect(screen.queryByText("claude-opus-4-5")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Claude Code/ }));
    await waitFor(() => {
      expect(screen.getByText("claude-opus-4-5")).toBeInTheDocument();
    });
    expect(screen.getByText("claude-haiku-4-5")).toBeInTheDocument();

    // Only the model with subagent tokens carries the subagent note.
    // The share text is assembled from sibling JSX nodes, so match on
    // the element's full textContent rather than a single text node.
    const shareText = (needle: string) => (_: string, el: Element | null) =>
      el?.tagName === "SPAN" && el.textContent === needle;
    // 2_021_000 / 7_021_000 ≈ 29%; 5_000_000 / 7_021_000 ≈ 71%.
    expect(
      screen.getByText(shareText("29% of tokens · subagents")),
    ).toBeInTheDocument();
    expect(screen.getByText(shareText("71% of tokens"))).toBeInTheDocument();

    // Collapsing hides them again.
    await userEvent.click(screen.getByRole("button", { name: /Claude Code/ }));
    await waitFor(() => {
      expect(screen.queryByText("claude-opus-4-5")).not.toBeInTheDocument();
    });
  });

  it("shows a quiet note instead of an empty chart when nothing ran", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        buckets: [],
        providers: [],
        totals: {
          metered_cost_usd: 0,
          plan_covered_cost_usd: 0,
          total_tokens: 0,
          cache_read_share: 0,
          session_count: 0,
          workspace_count: 0,
          cli_session_count: 0,
        },
      }),
    );
    render(<UsageSection />);
    await waitFor(() => {
      expect(
        screen.getByText("No agent activity in this period."),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText("Metered")).not.toBeInTheDocument();
  });

  it("surfaces a load failure inline", async () => {
    vi.mocked(usageSummary).mockRejectedValue(new Error("db locked"));
    render(<UsageSection />);
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

    render(<UsageSection />);
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
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("41%")).toBeInTheDocument();
    });
    expect(screen.getByText("88%")).toBeInTheDocument();
    expect(screen.getByText("5h")).toBeInTheDocument();
    expect(screen.getByText("week")).toBeInTheDocument();
    expect(screen.getByText("Max 20× · covered by plan")).toBeInTheDocument();
    expect(screen.getByText(/5h resets 16:40/)).toBeInTheDocument();
  });

  it("leaves a lane with no quota exactly as before", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Metered · your API keys")).toBeInTheDocument();
    });
    // No meters anywhere — an empty bar would imply a limit that does
    // not exist for OpenCode.
    expect(screen.queryByText("5h")).not.toBeInTheDocument();
    expect(screen.queryByText("week")).not.toBeInTheDocument();
  });

  it("follows a detected api_key auth mode in the lane's cost note", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        providers: [
          {
            ...summary().providers[0],
            // Backend detected a raw API key, so it classified Claude
            // as metered — the note must agree.
            billing: "metered",
          },
        ],
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
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("billed to your keys")).toBeInTheDocument();
    });
    expect(screen.getByText("Metered · your API keys")).toBeInTheDocument();
  });
});

describe("composition, breakdown and cost confidence", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
  });

  it("renders the five composition figures with their sub-notes", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Processed")).toBeInTheDocument();
    });
    expect(screen.getByText("Cached input")).toBeInTheDocument();
    expect(screen.getByText("Uncached input")).toBeInTheDocument();
    expect(screen.getByText("Output")).toBeInTheDocument();
    // "Cache savings" appears in the strip AND the confidence block —
    // both are specified, so assert presence rather than uniqueness.
    expect(screen.getAllByText("Cache savings").length).toBe(2);

    // Cache share is measured against observed INPUT, not all tokens.
    expect(screen.getByText("82% of input")).toBeInTheDocument();
    expect(screen.getByText("400K cache writes")).toBeInTheDocument();
    expect(screen.getByText("3.2× vs uncached list price")).toBeInTheDocument();
  });

  it("hides the reasoning note when no provider reported a split", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Output")).toBeInTheDocument();
    });
    expect(screen.queryByText(/reasoning/)).not.toBeInTheDocument();
  });

  it("shows the reasoning note once reasoning tokens exist", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        composition: { ...summary().composition, reasoning_tokens: 220_000 },
      }),
    );
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("includes 220K reasoning")).toBeInTheDocument();
    });
  });

  it("lists models flat across providers, em-dashing the unpriced one", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("gpt-5-codex")).toBeInTheDocument();
    });
    expect(screen.getByText("openrouter/kimi-k2")).toBeInTheDocument();
    // Unpriced → em-dash, and its share is expressed in TOKENS.
    expect(screen.getByText("—")).toBeInTheDocument();
    expect(screen.getByText("11% tok")).toBeInTheDocument();
  });

  it("switches the breakdown between Model and Day", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("gpt-5-codex")).toBeInTheDocument();
    });

    await userEvent.click(screen.getByRole("radio", { name: "Day" }));
    await waitFor(() => {
      expect(screen.queryByText("gpt-5-codex")).not.toBeInTheDocument();
    });
    // Day rows come from the same buckets the chart uses, newest first.
    expect(screen.getByText("Aug 7")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("radio", { name: "Model" }));
    await waitFor(() => {
      expect(screen.getByText("gpt-5-codex")).toBeInTheDocument();
    });
  });

  it("renders every cost-confidence row, zeros included", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        confidence: {
          provider_reported_share: 0,
          table_priced_share: 1,
          unpriced_token_share: 0,
          cache_savings_usd: 0,
        },
      }),
    );
    render(<UsageSection />);
    await waitFor(() => {
      expect(screen.getByText("Cost confidence")).toBeInTheDocument();
    });
    expect(screen.getByText("Provider reported")).toBeInTheDocument();
    expect(screen.getByText("Model priced")).toBeInTheDocument();
    expect(screen.getByText("Unpriced")).toBeInTheDocument();
    // Zero rows still render so the block keeps a stable shape.
    expect(screen.getByText("0%")).toBeInTheDocument();
    expect(screen.getByText("0% tok")).toBeInTheDocument();
  });

  it("offers a 90-day period that refetches", async () => {
    render(<UsageSection />);
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));
    await userEvent.click(screen.getByRole("radio", { name: "90 days" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("90d"));
  });

  it("refetches when the refresh button is pressed", async () => {
    render(<UsageSection />);
    await waitFor(() => expect(usageSummary).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledTimes(2));
    // Same period, not a period change.
    expect(vi.mocked(usageSummary).mock.calls.every(([p]) => p === "7d")).toBe(true);
  });
});

describe("terminal CLI folding", () => {
  beforeEach(() => {
    vi.mocked(usageSummary).mockReset();
    vi.mocked(usageExportCsv).mockReset();
    vi.mocked(usageScanCliLogs).mockReset();
    vi.mocked(usageSummary).mockResolvedValue(summary());
    vi.mocked(usageExportCsv).mockResolvedValue("bucket_start,provider\n");
    vi.mocked(usageScanCliLogs).mockResolvedValue({
      files_scanned: 34,
      sessions_found: 12,
      rows_added: 5,
      sessions_skipped_own: 7,
      reimported: false,
    });
  });

  it("renders the footer note with the CLIs' log paths", async () => {
    render(<UsageSection />);
    await waitFor(() => {
      expect(
        screen.getByText(/read from this machine's logs/),
      ).toBeInTheDocument();
    });
    expect(screen.getByText("~/.claude/projects")).toBeInTheDocument();
    expect(screen.getByText("~/.codex/sessions")).toBeInTheDocument();
  });

  /// There is no toggle any more: folding is unconditional, so the page
  /// scans on open and every query includes the imported rows.
  it("scans on open and always includes imports, with no toggle", async () => {
    render(<UsageSection />);
    await waitFor(() => expect(usageScanCliLogs).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(usageSummary).toHaveBeenCalledWith("7d"));
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    expect(screen.queryByText("excluded")).not.toBeInTheDocument();
  });

  /// THE count bug: the footer used to render the last scan's
  /// `sessions_found` (12 here, and far smaller on a warm incremental
  /// re-scan), not the period's real total.
  it("reports the period's total CLI sessions, not the last scan's new ones", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        totals: {
          metered_cost_usd: 6.44,
          plan_covered_cost_usd: 65.8,
          total_tokens: 8_421_000,
          cache_read_share: 0.7,
          session_count: 430,
          workspace_count: 4,
          cli_session_count: 430,
        },
      }),
    );
    render(<UsageSection />);
    await waitFor(() => {
      expect(
        screen.getByText(/Includes 430 terminal CLI sessions/),
      ).toBeInTheDocument();
    });
    // Emphatically not the scan report's figure.
    expect(screen.queryByText(/Includes 12 /)).not.toBeInTheDocument();
  });

  it("singularizes a lone CLI session", async () => {
    vi.mocked(usageSummary).mockResolvedValue(
      summary({
        totals: {
          metered_cost_usd: 0,
          plan_covered_cost_usd: 1,
          total_tokens: 100,
          cache_read_share: 0,
          session_count: 1,
          workspace_count: 0,
          cli_session_count: 1,
        },
      }),
    );
    render(<UsageSection />);
    await waitFor(() => {
      expect(
        screen.getByText(/Includes 1 terminal CLI session$/),
      ).toBeInTheDocument();
    });
  });

  it("exports the CSV for the visible period", async () => {
    Object.assign(URL, {
      createObjectURL: vi.fn(() => "blob:x"),
      revokeObjectURL: vi.fn(),
    });
    render(<UsageSection />);
    await waitFor(() => expect(usageSummary).toHaveBeenCalled());
    await userEvent.click(screen.getByRole("button", { name: "Export CSV" }));
    await waitFor(() => expect(usageExportCsv).toHaveBeenCalledWith("7d"));
  });

  /// A refresh must re-scan too, or a session that ran in a PTY tab
  /// since the page opened stays invisible until the next app start.
  it("re-scans on refresh", async () => {
    render(<UsageSection />);
    await waitFor(() => expect(usageScanCliLogs).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByRole("button", { name: "Refresh usage" }));
    await waitFor(() => expect(usageScanCliLogs).toHaveBeenCalledTimes(2));
  });
});
