import { useId, useMemo, useState } from "react";

import { cn } from "@/lib/utils";
import { Eyebrow } from "@/components/ui/eyebrow";
import { monotonePath, niceAxisMax, tickIndices, useMeasuredWidth } from "./usage-chart-axis";
import { formatPercent } from "./usage-format";

/** One provider's line: its per-bucket values under the active metric. */
export interface TrendSeries {
  key: string;
  label: string;
  /** Any CSS color, e.g. "var(--accent-ember)". */
  color: string;
  /** Opacity multiplier for neutral-tinted series (e.g. Codex at 0.55). */
  opacity?: number;
  /** One value per bucket under the active metric. */
  values: number[];
}

export interface TrendPoint {
  /** Short axis label. */
  label: string;
  /** Full label for the readout and, when short enough, the axis. */
  subLabel: string;
}

/** Plot margins. The y-axis labels sit in a left gutter sized to the
 *  widest one. */
const PAD_TOP = 10;
const PAD_BOTTOM = 24;
const PAD_X = 4;
const GRIDLINES = 4;
const TICK_LABEL_PX = 11;
/** Monospace advance at `TICK_LABEL_PX`, for sizing the y-axis gutter
 *  and keeping x labels inside the card without measuring text. */
const TICK_CHAR_PX = 6.8;
/** Longest full label that still fits between thinned ticks; "Aug 19"
 *  does, "Yesterday 13:00" does not. */
const TICK_FULL_LABEL_MAX_CHARS = 8;

/** A light wash under each line, so a quiet series still reads as an
 *  area without hiding the ones behind it. */
const AREA_FILL_OPACITY = 0.12;
const LINE_WIDTH_PX = 2;
/** Series other than the one pointed at elsewhere on the page. */
const UNHIGHLIGHTED_SERIES_OPACITY = 0.2;

function finite(v: number | undefined): number {
  return v !== undefined && Number.isFinite(v) && v > 0 ? v : 0;
}

/** Per-bucket totals across series, with missing and non-finite values as 0. */
function bucketTotals(series: TrendSeries[], count: number): number[] {
  return Array.from({ length: count }, (_, i) =>
    series.reduce((sum, s) => sum + finite(s.values[i]), 0),
  );
}

/** Index of the bucket with the largest total across series, or null when
 *  all are zero. */
export function busiestIndex(series: TrendSeries[], count: number): number | null {
  const totals = bucketTotals(series, count);
  let best: number | null = null;
  for (let i = 0; i < totals.length; i++) {
    // Strictly greater, so a tie keeps the earliest bucket.
    if (totals[i] > 0 && (best === null || totals[i] > totals[best])) best = i;
  }
  return best;
}

/** The bucket nearest a horizontal offset, with points spread edge to
 *  edge across the plot. */
function nearestIndex(offsetX: number, left: number, width: number, count: number): number | null {
  if (count === 0) return null;
  if (count === 1) return 0;
  const i = Math.round(((offsetX - left) / width) * (count - 1));
  return Math.min(count - 1, Math.max(0, i));
}

function seriesTotal(series: TrendSeries): number {
  return series.values.reduce((sum, v) => sum + finite(v), 0);
}

/**
 * One smooth line per provider over the period, each with a light wash
 * beneath it. Pointing at the chart (or using the arrow keys) reads out a
 * bucket: every provider's value and share, the total, and how it compares
 * with the period average.
 */
export function UsageTrendChart({
  series,
  points,
  height,
  hovered,
  onHover,
  highlightedSeries = null,
  formatValue,
  formatAxis = formatValue,
  secondaryValue,
  readoutExtra,
  overlay = null,
  ariaLabel,
  className,
}: {
  series: TrendSeries[];
  points: TrendPoint[];
  height: number;
  /** Controlled hovered bucket, shared with other parts of the page. */
  hovered: number | null;
  onHover: (index: number | null) => void;
  /** A provider being pointed at elsewhere (legend, a tool row): the other
   *  lines fade so its line stands out. */
  highlightedSeries?: string | null;
  /** Readout figures. */
  formatValue: (value: number) => string;
  /** Y-axis labels; defaults to `formatValue`. */
  formatAxis?: (value: number) => string;
  /** Muted secondary figure per series per bucket in the readout, e.g.
   *  tokens when the chart shows cost. */
  secondaryValue?: (seriesKey: string, index: number) => string | null;
  /** Extra rows at the bottom of the readout for a bucket. */
  readoutExtra?: (index: number) => React.ReactNode;
  /** Per-bucket value under the same metric, drawn as a dashed line — one
   *  model's part of every bucket, traced from the model list. */
  overlay?: number[] | null;
  ariaLabel: string;
  className?: string;
}) {
  const [ref, width] = useMeasuredWidth<HTMLDivElement>();
  const tooltipId = useId();
  const count = points.length;

  const totals = useMemo(() => bucketTotals(series, count), [series, count]);
  const average = useMemo(
    () => (count > 0 ? totals.reduce((a, b) => a + b, 0) / count : 0),
    [totals, count],
  );
  // Lines are not stacked, so the scale only has to reach the tallest one.
  const peak = useMemo(
    () => Math.max(0, ...series.flatMap((s) => s.values.map(finite))),
    [series],
  );
  const max = niceAxisMax(peak);

  // No data, no scale: an empty period shows bare gridlines rather than
  // an axis counting up to an invented 1.
  const axisLabels = useMemo(
    () =>
      peak > 0
        ? Array.from({ length: GRIDLINES + 1 }, (_, i) => formatAxis((max * i) / GRIDLINES))
        : [],
    [peak, max, formatAxis],
  );
  const gutter =
    axisLabels.length > 0
      ? Math.ceil(Math.max(...axisLabels.map((l) => l.length)) * TICK_CHAR_PX) + 10
      : 0;

  const plotLeft = PAD_X + gutter;
  const plotRight = Math.max(plotLeft + 1, width - PAD_X);
  const plotTop = PAD_TOP;
  const plotBottom = Math.max(plotTop + 1, height - PAD_BOTTOM);
  const plotWidth = plotRight - plotLeft;
  const plotHeight = plotBottom - plotTop;

  const xAt = (i: number) =>
    count <= 1 ? plotLeft + plotWidth / 2 : plotLeft + (i / (count - 1)) * plotWidth;

  // Geometry only depends on the data and the box, not on hover, so a
  // pointer sweep re-renders without re-deriving the curves.
  const { lines, overlayLine } = useMemo(() => {
    const x = (i: number) =>
      count <= 1 ? plotLeft + plotWidth / 2 : plotLeft + (i / (count - 1)) * plotWidth;
    const yAt = (v: number) => plotBottom - (finite(v) / max) * plotHeight;
    const toPoints = (values: (number | undefined)[]) =>
      Array.from({ length: count }, (_, i) => ({ x: x(i), y: yAt(values[i] ?? 0) }));
    // Heaviest first, so the smaller lines draw on top and stay visible.
    // An all-zero series has nothing to say and would only trace the baseline.
    const ordered = series
      .filter((s) => seriesTotal(s) > 0)
      .sort((a, b) => seriesTotal(b) - seriesTotal(a));
    const built = ordered.map((s) => {
      const pts = toPoints(s.values);
      const line = monotonePath(pts);
      const area =
        count >= 2 && line
          ? `${line} L${pts[count - 1].x},${plotBottom} L${pts[0].x},${plotBottom} Z`
          : "";
      return { series: s, pts, line, area };
    });
    const overlayPts = overlay ? toPoints(overlay) : null;
    return {
      lines: built,
      overlayLine: overlayPts ? monotonePath(overlayPts) : "",
    };
  }, [series, overlay, count, max, plotLeft, plotWidth, plotBottom, plotHeight]);

  const ticks = useMemo(() => tickIndices(count, plotWidth), [count, plotWidth]);
  // Prefer the full "Aug 19" over a bare "19" — a day number alone is
  // ambiguous once a 90-day axis spans months. Hourly "Today 13:00"
  // labels are too long and fall back to the compact "13:00" form.
  const fullTicks = ticks.every(
    (i) => (points[i]?.subLabel.length ?? 0) <= TICK_FULL_LABEL_MAX_CHARS,
  );

  const move = (index: number | null) => {
    if (index !== hovered) onHover(index);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (count === 0) return;
    let next: number | null | undefined;
    switch (event.key) {
      case "ArrowLeft":
        next = hovered === null ? count - 1 : Math.max(0, hovered - 1);
        break;
      case "ArrowRight":
        next = hovered === null ? count - 1 : Math.min(count - 1, hovered + 1);
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = count - 1;
        break;
      case "Escape":
        // Only claim the key when there is a readout to dismiss; with
        // nothing open, Escape goes on to close the page as usual.
        if (hovered === null) return;
        next = null;
        event.stopPropagation();
        break;
      default:
        return;
    }
    event.preventDefault();
    move(next);
  };

  const hoveredPoint = hovered === null ? null : (points[hovered] ?? null);
  const hoverX = hovered === null ? null : xAt(hovered);
  // Flip the readout to the left of the crosshair past the midpoint so it
  // never runs off the card's right edge.
  const readoutOnLeft = hoverX !== null && hoverX > width / 2;
  const readoutGap = 12;

  return (
    <div
      ref={ref}
      role="group"
      aria-label={ariaLabel}
      aria-describedby={hoveredPoint ? tooltipId : undefined}
      tabIndex={0}
      className={cn(
        "relative w-full select-none rounded-md focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
      style={{ height }}
      onPointerMove={(e) => {
        const rect = e.currentTarget.getBoundingClientRect();
        move(nearestIndex(e.clientX - rect.left, plotLeft, plotWidth, count));
      }}
      onPointerLeave={() => move(null)}
      onKeyDown={onKeyDown}
      onBlur={() => move(null)}
    >
      <svg
        width="100%"
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        aria-hidden
        className="block overflow-visible"
      >
        {/* Gridlines, each labelled with its value on the left. */}
        {Array.from({ length: GRIDLINES + 1 }, (_, i) => {
          const y = plotBottom - (i / GRIDLINES) * plotHeight;
          return (
            <g key={i}>
              <line
                x1={plotLeft}
                x2={plotRight}
                y1={y}
                y2={y}
                stroke="var(--border)"
                strokeOpacity={i === 0 ? 1 : 0.6}
                shapeRendering="crispEdges"
              />
              {axisLabels[i] !== undefined && (
                <text
                  x={plotLeft - 8}
                  y={y}
                  dominantBaseline="middle"
                  textAnchor="end"
                  fontSize={TICK_LABEL_PX}
                  className="font-mono tabular-nums"
                  fill="var(--muted-foreground)"
                  fillOpacity={0.95}
                >
                  {axisLabels[i]}
                </text>
              )}
            </g>
          );
        })}

        {/* Fills first, then every line, so no wash covers another line. */}
        {lines.map(({ series: s, area }) =>
          area ? (
            <path
              key={`area-${s.key}`}
              d={area}
              fill={s.color}
              fillOpacity={AREA_FILL_OPACITY * (s.opacity ?? 1)}
              className="transition-opacity duration-150"
              style={{
                opacity:
                  highlightedSeries != null && s.key !== highlightedSeries
                    ? UNHIGHLIGHTED_SERIES_OPACITY
                    : 1,
              }}
            />
          ) : null,
        )}
        {lines.map(({ series: s, line }) => (
          <path
            key={`line-${s.key}`}
            d={line}
            data-series={s.key}
            fill="none"
            stroke={s.color}
            strokeOpacity={s.opacity ?? 1}
            strokeWidth={LINE_WIDTH_PX}
            strokeLinejoin="round"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
            className="transition-opacity duration-150"
            style={{
              opacity:
                highlightedSeries != null && s.key !== highlightedSeries
                  ? UNHIGHLIGHTED_SERIES_OPACITY
                  : 1,
            }}
          />
        ))}

        {/* One model's part of every bucket, traced from the model list. */}
        {overlayLine && (
          <path
            data-overlay
            d={overlayLine}
            fill="none"
            stroke="var(--foreground)"
            strokeOpacity={0.85}
            strokeWidth={LINE_WIDTH_PX}
            strokeDasharray="4 4"
            vectorEffect="non-scaling-stroke"
          />
        )}

        {/* Crosshair and the points it passes through. */}
        {hoverX !== null && hovered !== null && (
          <g data-crosshair>
            <line
              x1={hoverX}
              x2={hoverX}
              y1={plotTop}
              y2={plotBottom}
              stroke="var(--muted-foreground)"
              strokeOpacity={0.8}
              shapeRendering="crispEdges"
            />
            {lines.map(({ series: s, pts }) => {
              const p = pts[hovered];
              if (!p) return null;
              const dimmed = highlightedSeries != null && s.key !== highlightedSeries;
              return (
                <circle
                  key={s.key}
                  cx={p.x}
                  cy={p.y}
                  r={3.5}
                  fill={s.color}
                  fillOpacity={(s.opacity ?? 1) * (dimmed ? UNHIGHLIGHTED_SERIES_OPACITY : 1)}
                  stroke="var(--background)"
                  strokeWidth={1.5}
                />
              );
            })}
          </g>
        )}

        {ticks.map((i) => {
          const text = (fullTicks ? points[i]?.subLabel : points[i]?.label) ?? "";
          const anchor = count <= 1 ? "middle" : i === 0 ? "start" : i === count - 1 ? "end" : "middle";
          return (
            <text
              key={i}
              x={xAt(i)}
              y={height - 6}
              textAnchor={anchor}
              fontSize={TICK_LABEL_PX}
              className="font-mono uppercase tracking-[0.06em]"
              fill="var(--foreground)"
              fillOpacity={hovered === i ? 1 : 0.7}
            >
              {text}
            </text>
          );
        })}
      </svg>

      {hoveredPoint && hoverX !== null && hovered !== null && (
        <div
          id={tooltipId}
          role="tooltip"
          className="pointer-events-none absolute top-1 z-10 min-w-[200px] rounded-md border border-border/70 bg-popover/95 px-3 py-2 shadow-lg backdrop-blur-sm"
          style={
            readoutOnLeft
              ? { right: Math.max(0, width - hoverX + readoutGap) }
              : { left: Math.min(hoverX + readoutGap, Math.max(0, width - 220)) }
          }
        >
          <Eyebrow className="mb-1.5">{hoveredPoint.subLabel}</Eyebrow>
          <Readout
            series={series}
            index={hovered}
            total={totals[hovered] ?? 0}
            average={average}
            formatValue={formatValue}
            secondaryValue={secondaryValue}
          />
          {readoutExtra?.(hovered)}
        </div>
      )}
    </div>
  );
}

/** The figures for one bucket: providers largest-first with their share,
 *  then the total and how the bucket compares with the period. */
function Readout({
  series,
  index,
  total,
  average,
  formatValue,
  secondaryValue,
}: {
  series: TrendSeries[];
  index: number;
  total: number;
  average: number;
  formatValue: (value: number) => string;
  secondaryValue?: (seriesKey: string, index: number) => string | null;
}) {
  const rows = series
    .map((s) => ({ s, value: finite(s.values[index]) }))
    // Array sort is stable, so equal buckets keep the drawing order.
    .sort((a, b) => b.value - a.value);
  const secondary = rows.map(({ s, value }) =>
    value > 0 && secondaryValue ? secondaryValue(s.key, index) : null,
  );
  const showSecondary = secondary.some((v) => v);
  const delta = average > 0 ? total / average - 1 : null;
  const deltaPercent = delta === null ? 0 : Math.round(delta * 100);

  return (
    <>
      <div
        className="grid items-center gap-x-3 gap-y-1 text-label"
        style={{
          gridTemplateColumns: showSecondary
            ? "minmax(0,1fr) auto auto auto"
            : "minmax(0,1fr) auto auto",
        }}
      >
        {rows.map(({ s, value }, k) => {
          const empty = value <= 0;
          return (
            <div key={s.key} data-readout-row={s.key} className="contents">
              <span
                className={cn(
                  "inline-flex min-w-0 items-center gap-2",
                  empty ? "text-muted-foreground/60" : "text-foreground/85",
                )}
              >
                <span
                  className="size-2 shrink-0 rounded-sm"
                  style={{
                    background: s.color,
                    opacity: (s.opacity ?? 1) * (empty ? 0.4 : 1),
                  }}
                  aria-hidden
                />
                <span className="truncate">{s.label}</span>
              </span>
              <span
                className={cn(
                  "text-right font-mono tabular-nums",
                  empty ? "text-muted-foreground/60" : "text-foreground",
                )}
              >
                {empty ? "—" : formatValue(value)}
              </span>
              {showSecondary && (
                <span className="text-right font-mono text-label tabular-nums text-foreground/70">
                  {secondary[k] ?? ""}
                </span>
              )}
              <span className="text-right font-mono text-label tabular-nums text-foreground/70">
                {empty || total <= 0 ? "" : formatPercent(value / total)}
              </span>
            </div>
          );
        })}
      </div>
      {series.length > 1 && (
        <p className="mt-1.5 flex items-center justify-between gap-4 border-t border-border/50 pt-1.5 text-label">
          <span className="text-muted-foreground">Total</span>
          <span className="font-mono font-semibold tabular-nums">{formatValue(total)}</span>
        </p>
      )}
      {delta !== null && (
        <p className="mt-1 flex items-center justify-between gap-4 text-label text-foreground/70">
          <span>vs period average</span>
          <span className="font-mono tabular-nums">
            {deltaPercent > 0 ? "+" : deltaPercent < 0 ? "−" : ""}
            {Math.abs(deltaPercent)}%
          </span>
        </p>
      )}
    </>
  );
}

/** A provider's line on its own scale, for a tool row. Shares the
 *  hovered bucket with the main chart, so pointing at either marks the
 *  same day in both. */
export function UsageSparkline({
  values,
  color,
  opacity = 1,
  height,
  hovered,
  onHover,
  ariaLabel,
  describe,
  className,
}: {
  values: number[];
  color: string;
  opacity?: number;
  height: number;
  hovered: number | null;
  onHover: (index: number | null) => void;
  /** Accessible label of the sparkline. */
  ariaLabel: string;
  /** Text for the hovered point, e.g. "Oct 15 · $18.40 · 1.9M tokens". */
  describe: (index: number) => string;
  className?: string;
}) {
  const [ref, width] = useMeasuredWidth<HTMLSpanElement>();
  const gradientId = useId();
  const count = values.length;
  const top = 3;
  const bottom = height - 2;

  // Normalised to this line's own peak: a row shows its provider's rhythm,
  // not its size against the others (the main chart does that).
  const { pts, line, area } = useMemo(() => {
    const max = Math.max(0, ...values.map((v) => finite(v))) || 1;
    const points = values.map((v, i) => ({
      x: count <= 1 ? width / 2 : (i / (count - 1)) * width,
      y: bottom - (finite(v) / max) * (bottom - top),
    }));
    const d = monotonePath(points);
    return {
      pts: points,
      line: d,
      area: count >= 2 && d ? `${d} L${width},${bottom} L0,${bottom} Z` : "",
    };
  }, [values, width, count, bottom]);

  // Every sparkline shares the hovered day, but only the one under the
  // pointer speaks; the rest just mark the point.
  const [pointerInside, setPointerInside] = useState(false);
  const hoveredText =
    pointerInside && hovered !== null && hovered < count ? describe(hovered) : null;
  const point = hovered === null ? null : (pts[hovered] ?? null);
  const center = point?.x ?? 0;
  // Keep the label inside the row near either end instead of centring it
  // off the edge.
  const align = center < width * 0.2 ? "start" : center > width * 0.8 ? "end" : "center";

  return (
    <span
      ref={ref}
      role="img"
      aria-label={ariaLabel}
      className={cn("relative block w-full", className)}
      style={{ height }}
      onPointerMove={(e) => {
        setPointerInside(true);
        const rect = e.currentTarget.getBoundingClientRect();
        const i = nearestIndex(e.clientX - rect.left, 0, width, count);
        if (i !== hovered) onHover(i);
      }}
      onPointerLeave={() => {
        setPointerInside(false);
        onHover(null);
      }}
    >
      <svg
        width="100%"
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        aria-hidden
        className="block overflow-visible"
        opacity={opacity}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={0.25} />
            <stop offset="100%" stopColor={color} stopOpacity={0} />
          </linearGradient>
        </defs>
        {area && <path d={area} fill={`url(#${gradientId})`} />}
        {line && (
          <path
            d={line}
            fill="none"
            stroke={color}
            strokeWidth={1.5}
            strokeLinejoin="round"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
          />
        )}
        {point && (
          <circle
            data-day={hovered}
            cx={point.x}
            cy={point.y}
            r={3}
            fill={color}
            stroke="var(--background)"
            strokeWidth={1.5}
          />
        )}
      </svg>
      {hoveredText && (
        <span
          role="tooltip"
          className="pointer-events-none absolute bottom-full z-10 mb-1 whitespace-nowrap rounded-sm border border-border/70 bg-popover px-1.5 py-0.5 font-mono text-label tabular-nums text-popover-foreground shadow-md"
          style={
            align === "start"
              ? { left: 0 }
              : align === "end"
                ? { right: 0 }
                : { left: center, transform: "translateX(-50%)" }
          }
        >
          {hoveredText}
        </span>
      )}
    </span>
  );
}
