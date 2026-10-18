/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { CategoryCost, FlatModelUsage, UsageComposition } from "@/tauri/commands";
import { TooltipProvider } from "@/components/ui/tooltip";
import { TermHint } from "./term-hint";
import { UsageTokenTypes, modelTokenTypeRows, tokenTypeRows } from "./usage-token-types";

afterEach(() => cleanup());

function composition(overrides: Partial<UsageComposition> = {}): UsageComposition {
  return {
    processed_tokens: 12_500_000,
    cache_read_tokens: 7_990_000,
    cache_read_share_of_input: 0.73,
    input_tokens: 2_000_000,
    cache_write_tokens: 960_000,
    output_tokens: 1_550_000,
    reasoning_tokens: 0,
    cache_savings_usd: 40,
    cache_savings_multiplier: 3.2,
    ...overrides,
  };
}

function categoryCost(overrides: Partial<CategoryCost> = {}): CategoryCost {
  return { input: 20, cache_read: 5.19, cache_write: 12, output: 57.23, unsplit: 0, ...overrides };
}

function model(overrides: Partial<FlatModelUsage> = {}): FlatModelUsage {
  return {
    provider: "codex",
    model: "gpt-5-codex",
    tokens: 4_000_000,
    cost_usd: 10,
    priced: true,
    provider_reported: false,
    price_overridden: false,
    input_tokens: 500_000,
    output_tokens: 1_000_000,
    cache_read_tokens: 1_500_000,
    cache_write_tokens: 1_000_000,
    reasoning_tokens: 0,
    unpriced_tokens: 0,
    session_count: 1,
    category_cost: { input: 1, output: 6, cache_read: 1, cache_write: 2, unsplit: 0 },
    rates: null,
    cost_sources: { provider_usd: 0, table_usd: 10, override_usd: 0 },
    cache_savings: { savings_usd: 0, uncached_usd: 0, cached_usd: 0 },
    buckets: [],
    ...overrides,
  };
}

const sum = (values: number[]) => values.reduce((a, b) => a + b, 0);

describe("tokenTypeRows", () => {
  it("leaves out unsplit cost when there is none", () => {
    const rows = tokenTypeRows(composition(), categoryCost());
    expect(rows.map((row) => row.key)).toEqual(["input", "cache_read", "cache_write", "output"]);
  });

  it("adds an unsplit row, with no tokens, when the provider reported unsplit cost", () => {
    const rows = tokenTypeRows(composition(), categoryCost({ unsplit: 1.2 }));
    const unsplit = rows.find((row) => row.key === "unsplit");
    expect(unsplit).toMatchObject({ label: "Not split by type", tokens: null, cost: 1.2 });
  });

  it("carries the period's tokens and cost into rows that sum to the totals", () => {
    const c = composition();
    const cost = categoryCost({ unsplit: 3 });
    const rows = tokenTypeRows(c, cost);
    expect(sum(rows.map((row) => row.tokens ?? 0))).toBe(
      c.input_tokens + c.cache_read_tokens + c.cache_write_tokens + c.output_tokens,
    );
    expect(sum(rows.map((row) => row.cost))).toBeCloseTo(
      cost.input + cost.cache_read + cost.cache_write + cost.output + cost.unsplit,
    );
  });

  it("names cache reads plainly and notes their share of input", () => {
    const read = tokenTypeRows(composition(), categoryCost()).find(
      (row) => row.key === "cache_read",
    );
    expect(read).toMatchObject({
      label: "Reused context",
      technical: "cache read",
      note: "73% of all input",
    });
  });

  it("notes reasoning on output only when a provider reported some", () => {
    const output = (reasoning: number) =>
      tokenTypeRows(composition({ reasoning_tokens: reasoning }), categoryCost()).find(
        (row) => row.key === "output",
      );
    expect(output(0)?.note).toBeUndefined();
    expect(output(301_000)?.note).toBe("incl. 301K reasoning");
  });
});

describe("modelTokenTypeRows", () => {
  it("uses the model's own split and cache-read share of its input", () => {
    const rows = modelTokenTypeRows(model({ reasoning_tokens: 200_000 }));
    expect(rows.map((row) => [row.key, row.tokens, row.cost])).toEqual([
      ["input", 500_000, 1],
      ["cache_read", 1_500_000, 1],
      ["cache_write", 1_000_000, 2],
      ["output", 1_000_000, 6],
    ]);
    // 1.5M reads of 3M observed input.
    expect(rows[1].note).toBe("50% of all input");
    expect(rows[3].note).toBe("incl. 200K reasoning");
  });
});

describe("UsageTokenTypes", () => {
  function renderRows(rows = tokenTypeRows(composition(), categoryCost({ unsplit: 5.58 }))) {
    return render(<UsageTokenTypes rows={rows} emphasis="cost" />, { wrapper: TooltipProvider });
  }

  it("shows plain labels, technical names and both totals", () => {
    renderRows();
    for (const label of ["New input", "Reused context", "Saved to cache", "Output", "Not split by type"]) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }
    expect(screen.getByText("uncached input")).toBeInTheDocument();
    expect(screen.getByText("cache read")).toBeInTheDocument();
    expect(screen.getByText("cache write")).toBeInTheDocument();
    // 2M + 7.99M + 0.96M + 1.55M tokens; $20 + $5.19 + $12 + $57.23 + $5.58.
    expect(screen.getByText("12.5M")).toBeInTheDocument();
    expect(screen.getByText("$100.00")).toBeInTheDocument();
  });

  it("puts each kind's share of tokens and share of cost on the same scale", () => {
    renderRows();
    // 7.99M of 12.5M tokens, $5.19 of $100.
    expect(screen.getByText("7.99M · 64%")).toBeInTheDocument();
    expect(screen.getByText("$5.19 · 5.2%")).toBeInTheDocument();
    // Output: a small share of the tokens, most of the bill.
    expect(screen.getByText("1.55M · 12%")).toBeInTheDocument();
    expect(screen.getByText("$57.23 · 57%")).toBeInTheDocument();
    // Unsplit cost has no tokens to count.
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("reads each row out as one sentence", () => {
    renderRows();
    expect(
      screen.getByText("Reused context: 7.99M tokens, 64% of tokens; $5.19, 5.2% of cost."),
    ).toHaveClass("sr-only");
    expect(
      screen.getByText("Not split by type: no tokens counted; $5.58, 5.6% of cost."),
    ).toBeInTheDocument();
  });

  it("dashes the cost side when nothing was priced", () => {
    render(
      <UsageTokenTypes
        rows={modelTokenTypeRows(
          model({
            priced: false,
            category_cost: { input: 0, output: 0, cache_read: 0, cache_write: 0, unsplit: 0 },
          }),
        )}
        emphasis="tokens"
        compact
      />,
      { wrapper: TooltipProvider },
    );
    // The cost total plus one dash per row.
    expect(screen.getAllByText("—")).toHaveLength(5);
    expect(screen.getByText("1.5M · 38%")).toBeInTheDocument();
    expect(screen.queryByText("share of all cost")).not.toBeInTheDocument();
  });

  it("explains a term when its label takes keyboard focus", async () => {
    renderRows();
    await userEvent.tab();
    expect(screen.getByRole("button", { name: "New input" })).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Prompt text the model read fresh, at the full input price.",
    );
  });
});

describe("TermHint", () => {
  it("opens its hint on focus", async () => {
    render(<TermHint hint="Explained.">Jargon</TermHint>, { wrapper: TooltipProvider });
    screen.getByRole("button", { name: "Jargon" }).focus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Explained.");
  });
});
