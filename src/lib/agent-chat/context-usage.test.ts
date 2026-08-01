import { describe, expect, it } from "vitest";

import type { ContextUsageSnapshot } from "@/tauri/events";
import type { ChatModelInfo } from "@/tauri/types";

import {
  deriveContextUsageDisplay,
  formatContextPercentage,
  formatContextTokens,
  resolveContextWindowTokens,
} from "./context-usage";

function model(overrides: Partial<ChatModelInfo> = {}): ChatModelInfo {
  return {
    id: "m1",
    label: "Model One",
    description: null,
    effort_levels: [],
    default_effort: null,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: false,
    supports_fast_mode: false,
    supports_images: false,
    is_free: false,
    ...overrides,
  } as ChatModelInfo;
}

describe("formatContextTokens", () => {
  it("prints whole numbers below 1k", () => {
    expect(formatContextTokens(0)).toBe("0");
    expect(formatContextTokens(42)).toBe("42");
    expect(formatContextTokens(999)).toBe("999");
  });

  it("keeps one decimal in the 1k–10k band and strips a trailing .0", () => {
    expect(formatContextTokens(1_000)).toBe("1k");
    expect(formatContextTokens(1_400)).toBe("1.4k");
    expect(formatContextTokens(9_499)).toBe("9.5k");
  });

  it("rounds to whole thousands from 10k up", () => {
    // 9_999 rounds up into the next band's shape but stays in `k`.
    expect(formatContextTokens(9_999)).toBe("10k");
    expect(formatContextTokens(10_000)).toBe("10k");
    expect(formatContextTokens(224_000)).toBe("224k");
    expect(formatContextTokens(224_400)).toBe("224k");
    // Band edge: still below 1m, so still expressed in thousands.
    expect(formatContextTokens(999_999)).toBe("1000k");
  });

  it("switches to millions at 1m with one decimal", () => {
    expect(formatContextTokens(1_000_000)).toBe("1m");
    expect(formatContextTokens(1_500_000)).toBe("1.5m");
    expect(formatContextTokens(3_100_000)).toBe("3.1m");
  });

  it("never blanks out on missing or malformed input", () => {
    expect(formatContextTokens(null)).toBe("0");
    expect(formatContextTokens(Number.NaN)).toBe("0");
    expect(formatContextTokens(Number.POSITIVE_INFINITY)).toBe("0");
    expect(formatContextTokens(-5)).toBe("0");
  });
});

describe("formatContextPercentage", () => {
  it("keeps one decimal below 10%", () => {
    expect(formatContextPercentage(3.24)).toBe("3.2%");
    expect(formatContextPercentage(0)).toBe("0%");
    expect(formatContextPercentage(5)).toBe("5%");
    expect(formatContextPercentage(9.94)).toBe("9.9%");
  });

  it("rounds to a whole percent at 10% and above", () => {
    expect(formatContextPercentage(10)).toBe("10%");
    expect(formatContextPercentage(22.4)).toBe("22%");
    expect(formatContextPercentage(99.6)).toBe("100%");
  });

  it("returns null when there is no percentage to show", () => {
    expect(formatContextPercentage(null)).toBeNull();
    expect(formatContextPercentage(Number.NaN)).toBeNull();
    expect(formatContextPercentage(Number.POSITIVE_INFINITY)).toBeNull();
  });
});

describe("resolveContextWindowTokens", () => {
  const withOptions = model({
    context_window_options: [
      {
        value: "200k",
        label: "200K",
        is_default: true,
        context_window_tokens: 200_000,
      },
      {
        value: "1m",
        label: "1M",
        is_default: false,
        context_window_tokens: 1_000_000,
      },
    ],
    max_context_tokens: 128_000,
  });

  it("prefers the selected option's numeric size", () => {
    expect(resolveContextWindowTokens(withOptions, "1m")).toBe(1_000_000);
  });

  it("falls back to the default option when nothing is selected", () => {
    expect(resolveContextWindowTokens(withOptions, null)).toBe(200_000);
  });

  it("falls back to the default option when the selection is unknown", () => {
    expect(resolveContextWindowTokens(withOptions, "nope")).toBe(200_000);
  });

  it("falls back to the default option when the selection has no size", () => {
    const m = model({
      context_window_options: [
        {
          value: "200k",
          label: "200K",
          is_default: true,
          context_window_tokens: 200_000,
        },
        { value: "1m", label: "1M", is_default: false },
      ],
    });
    expect(resolveContextWindowTokens(m, "1m")).toBe(200_000);
  });

  it("falls back to the model's own advertised window last", () => {
    const m = model({ max_context_tokens: 128_000 });
    expect(resolveContextWindowTokens(m, "1m")).toBe(128_000);
  });

  it("returns null rather than guessing when nothing is known", () => {
    expect(resolveContextWindowTokens(model(), null)).toBeNull();
    expect(resolveContextWindowTokens(null, "1m")).toBeNull();
    expect(resolveContextWindowTokens(undefined, null)).toBeNull();
  });

  it("ignores non-positive sizes", () => {
    const m = model({
      context_window_options: [
        { value: "x", label: "X", is_default: true, context_window_tokens: 0 },
      ],
      max_context_tokens: -1,
    });
    expect(resolveContextWindowTokens(m, "x")).toBeNull();
  });
});

describe("deriveContextUsageDisplay", () => {
  const snap = (o: Partial<ContextUsageSnapshot> = {}): ContextUsageSnapshot =>
    ({ used_tokens: 44_000, ...o }) as ContextUsageSnapshot;

  it("computes a percentage from the snapshot's own window", () => {
    const d = deriveContextUsageDisplay(snap({ max_tokens: 200_000 }));
    expect(d.usedTokens).toBe(44_000);
    expect(d.maxTokens).toBe(200_000);
    expect(d.usedPercentage).toBe(22);
  });

  it("uses the seed only when the snapshot has no window of its own", () => {
    const seeded = deriveContextUsageDisplay(snap(), 200_000);
    expect(seeded.maxTokens).toBe(200_000);
    expect(seeded.usedPercentage).toBe(22);

    const reported = deriveContextUsageDisplay(
      snap({ max_tokens: 400_000 }),
      200_000,
    );
    expect(reported.maxTokens).toBe(400_000);
    expect(reported.usedPercentage).toBe(11);
  });

  it("yields no percentage when the window is unknown", () => {
    const d = deriveContextUsageDisplay(snap({ max_tokens: null }), null);
    expect(d.maxTokens).toBeNull();
    expect(d.usedPercentage).toBeNull();
    expect(d.usedTokens).toBe(44_000);
  });

  it("clamps the percentage into 0–100", () => {
    expect(
      deriveContextUsageDisplay(
        snap({ used_tokens: 500_000, max_tokens: 200_000 }),
      ).usedPercentage,
    ).toBe(100);
    expect(
      deriveContextUsageDisplay(snap({ used_tokens: 0, max_tokens: 200_000 }))
        .usedPercentage,
    ).toBe(0);
  });

  it("treats a zero or negative window as unknown", () => {
    expect(
      deriveContextUsageDisplay(snap({ max_tokens: 0 })).usedPercentage,
    ).toBeNull();
    expect(
      deriveContextUsageDisplay(snap({ max_tokens: -10 })).maxTokens,
    ).toBeNull();
  });

  it("surfaces the lifetime total only once it exceeds live usage", () => {
    expect(
      deriveContextUsageDisplay(snap({ total_processed_tokens: 91_000 }))
        .totalProcessedTokens,
    ).toBe(91_000);
    expect(
      deriveContextUsageDisplay(snap({ total_processed_tokens: 44_000 }))
        .totalProcessedTokens,
    ).toBeNull();
    expect(
      deriveContextUsageDisplay(snap({ total_processed_tokens: 10 }))
        .totalProcessedTokens,
    ).toBeNull();
    expect(deriveContextUsageDisplay(snap()).totalProcessedTokens).toBeNull();
  });

  it("reports auto-compaction only on an explicit true", () => {
    expect(deriveContextUsageDisplay(snap()).compactsAutomatically).toBe(false);
    expect(
      deriveContextUsageDisplay(snap({ compacts_automatically: null }))
        .compactsAutomatically,
    ).toBe(false);
    expect(
      deriveContextUsageDisplay(snap({ compacts_automatically: true }))
        .compactsAutomatically,
    ).toBe(true);
  });

  it("degrades safely on a missing or malformed snapshot", () => {
    const none = deriveContextUsageDisplay(null, 200_000);
    expect(none.usedTokens).toBe(0);
    expect(none.usedPercentage).toBeNull();

    const bad = deriveContextUsageDisplay(
      snap({ used_tokens: Number.NaN, max_tokens: 200_000 }),
    );
    expect(bad.usedTokens).toBe(0);
    expect(bad.usedPercentage).toBe(0);
  });
});
