/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

import {
  UsageSparkline,
  UsageTrendChart,
  busiestIndex,
  type TrendPoint,
  type TrendSeries,
} from "./usage-trend-chart";

afterEach(() => cleanup());

const POINTS: TrendPoint[] = [
  { label: "13", subLabel: "Oct 13" },
  { label: "14", subLabel: "Oct 14" },
  { label: "15", subLabel: "Oct 15" },
];

const SERIES: TrendSeries[] = [
  { key: "claude", label: "Claude", color: "var(--accent-ember)", values: [2, 6, 0] },
  {
    key: "codex",
    label: "Codex",
    color: "var(--foreground)",
    opacity: 0.55,
    values: [1, 2, 3],
  },
];

const money = (v: number) => `$${v.toFixed(2)}`;

function renderChart(props: Partial<React.ComponentProps<typeof UsageTrendChart>> = {}) {
  const onHover = vi.fn();
  const utils = render(
    <UsageTrendChart
      series={SERIES}
      points={POINTS}
      height={200}
      hovered={null}
      onHover={onHover}
      formatValue={money}
      ariaLabel="Cost per day by provider"
      {...props}
    />,
  );
  return { ...utils, onHover, chart: screen.getByRole("group", { name: "Cost per day by provider" }) };
}

describe("busiestIndex", () => {
  it("picks the bucket with the largest total across series", () => {
    // Per-series peaks are elsewhere (codex peaks at 2), but the stack
    // peaks at 1: 6 + 2.
    expect(busiestIndex(SERIES, 3)).toBe(1);
  });

  it("keeps the first bucket on a tie", () => {
    const tied: TrendSeries[] = [
      { key: "a", label: "A", color: "var(--foreground)", values: [1, 3, 3] },
    ];
    expect(busiestIndex(tied, 3)).toBe(1);
  });

  it("is null when nothing was used", () => {
    const empty: TrendSeries[] = [
      { key: "a", label: "A", color: "var(--foreground)", values: [0, 0, 0] },
    ];
    expect(busiestIndex(empty, 3)).toBeNull();
    expect(busiestIndex([], 0)).toBeNull();
  });
});

describe("UsageTrendChart", () => {
  it("reads out each provider, the total and each share for the hovered bucket", () => {
    renderChart({
      hovered: 1,
      secondaryValue: (key) => (key === "claude" ? "1.2M tok" : "300K tok"),
      readoutExtra: () => <p>Top model Opus</p>,
    });
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("Oct 14");

    const claude = tooltip.querySelector('[data-readout-row="claude"]') as HTMLElement;
    const codex = tooltip.querySelector('[data-readout-row="codex"]') as HTMLElement;
    expect(within(claude).getByText("$6.00")).toBeInTheDocument();
    expect(within(claude).getByText("75%")).toBeInTheDocument();
    expect(within(claude).getByText("1.2M tok")).toBeInTheDocument();
    expect(within(codex).getByText("$2.00")).toBeInTheDocument();
    expect(within(codex).getByText("25%")).toBeInTheDocument();
    expect(within(tooltip).getByText("Total")).toBeInTheDocument();
    expect(within(tooltip).getByText("$8.00")).toBeInTheDocument();
    // Average of 3, 8, 3 is 4.67; 8 is 71% above it.
    expect(tooltip).toHaveTextContent("vs period average+71%");
    expect(within(tooltip).getByText("Top model Opus")).toBeInTheDocument();
  });

  it("lists providers largest-first and an idle one muted", () => {
    renderChart({ hovered: 2 });
    const tooltip = screen.getByRole("tooltip");
    const rows = [...tooltip.querySelectorAll("[data-readout-row]")].map((r) =>
      r.getAttribute("data-readout-row"),
    );
    expect(rows).toEqual(["codex", "claude"]);
    const claude = tooltip.querySelector('[data-readout-row="claude"]') as HTMLElement;
    expect(within(claude).getByText("—")).toBeInTheDocument();
    // Average 4.67 against 3 is 36% below.
    expect(tooltip).toHaveTextContent("−36%");
  });

  it("sets the hovered bucket from the pointer and clears it on leave", () => {
    const { chart, onHover } = renderChart();
    // jsdom has no layout: the chart lays out at the 640px fallback with
    // its rect at 0, so the last third of the width is the last bucket.
    fireEvent.pointerMove(chart, { clientX: 630 });
    expect(onHover).toHaveBeenLastCalledWith(2);
    fireEvent.pointerLeave(chart);
    // Hover is controlled and still null here, so leaving is a no-op.
    expect(onHover).toHaveBeenCalledTimes(1);
  });

  it("steps through buckets with the arrow keys", () => {
    const { chart, onHover, rerender } = renderChart();
    fireEvent.keyDown(chart, { key: "ArrowLeft" });
    expect(onHover).toHaveBeenLastCalledWith(2);

    rerender(
      <UsageTrendChart
        series={SERIES}
        points={POINTS}
        height={200}
        hovered={1}
        onHover={onHover}
        formatValue={money}
        ariaLabel="Cost per day by provider"
      />,
    );
    fireEvent.keyDown(chart, { key: "ArrowLeft" });
    expect(onHover).toHaveBeenLastCalledWith(0);
    fireEvent.keyDown(chart, { key: "ArrowRight" });
    expect(onHover).toHaveBeenLastCalledWith(2);
    fireEvent.keyDown(chart, { key: "Home" });
    expect(onHover).toHaveBeenLastCalledWith(0);
    fireEvent.keyDown(chart, { key: "End" });
    expect(onHover).toHaveBeenLastCalledWith(2);
  });

  it("clears the readout on Escape without letting the page close", () => {
    const pageEscape = vi.fn();
    const { chart, onHover } = renderChart({ hovered: 1 });
    window.addEventListener("keydown", pageEscape);
    try {
      const notCancelled = fireEvent.keyDown(chart, { key: "Escape" });
      expect(onHover).toHaveBeenLastCalledWith(null);
      expect(notCancelled).toBe(false);
      expect(pageEscape).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", pageEscape);
    }
  });

  it("dims every series but the highlighted one", () => {
    const { container } = renderChart({ highlightedSeries: "codex" });
    const claude = container.querySelector('path[data-series="claude"]') as SVGElement;
    const codex = container.querySelector('path[data-series="codex"]') as SVGElement;
    expect(claude.style.opacity).toBe("0.2");
    expect(codex.style.opacity).toBe("1");
  });

  it("draws a crosshair with a point on each line for the hovered bucket", () => {
    const { container } = renderChart({ hovered: 0 });
    const crosshair = container.querySelector("[data-crosshair]");
    expect(crosshair).not.toBeNull();
    expect(crosshair!.querySelectorAll("circle")).toHaveLength(2);
  });

  it("draws no crosshair while nothing is hovered", () => {
    const { container } = renderChart();
    expect(container.querySelector("[data-crosshair]")).toBeNull();
  });

  it("draws one model's trace as a dashed line only when given", () => {
    const { container, rerender, onHover } = renderChart();
    expect(container.querySelector("[data-overlay]")).toBeNull();
    rerender(
      <UsageTrendChart
        series={SERIES}
        points={POINTS}
        height={200}
        hovered={null}
        onHover={onHover}
        formatValue={money}
        ariaLabel="Cost per day by provider"
        overlay={[1, 0, 2]}
      />,
    );
    const overlay = container.querySelector("[data-overlay]");
    expect(overlay).not.toBeNull();
    expect(overlay!.getAttribute("stroke-dasharray")).toBe("4 4");
  });

  it("shows bare gridlines with no axis labels for an empty period", () => {
    const { container } = renderChart({
      series: [{ key: "a", label: "A", color: "var(--foreground)", values: [0, 0, 0] }],
    });
    expect(container.querySelectorAll("line")).toHaveLength(5);
    expect(container.querySelectorAll("[data-series]")).toHaveLength(0);
    // Only the three x tick labels remain.
    expect(container.querySelectorAll("text")).toHaveLength(3);
  });
});

describe("UsageSparkline", () => {
  function renderStrip(hovered: number | null) {
    const onHover = vi.fn();
    render(
      <UsageSparkline
        values={[1, 4, 2]}
        color="var(--accent-ember)"
        height={24}
        hovered={hovered}
        onHover={onHover}
        ariaLabel="Claude per day"
        describe={(i) => `${POINTS[i].subLabel} · $${i}.00`}
      />,
    );
    return { onHover, strip: screen.getByRole("img", { name: "Claude per day" }) };
  }

  it("describes the hovered day only while the pointer is on this sparkline", () => {
    const { strip } = renderStrip(1);
    // A day hovered elsewhere on the page marks the point but says nothing.
    expect(screen.queryByRole("tooltip")).toBeNull();
    fireEvent.pointerMove(strip, { clientX: 10 });
    expect(screen.getByRole("tooltip")).toHaveTextContent("Oct 14 · $1.00");
    fireEvent.pointerLeave(strip);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("marks the shared hovered day and reports pointer movement", () => {
    const { strip, onHover } = renderStrip(null);
    expect(screen.queryByRole("tooltip")).toBeNull();
    fireEvent.pointerMove(strip, { clientX: 10 });
    expect(onHover).toHaveBeenLastCalledWith(0);
    fireEvent.pointerLeave(strip);
    expect(onHover).toHaveBeenLastCalledWith(null);
  });
});
