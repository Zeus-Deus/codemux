import { useId, useLayoutEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";

/** One line on the chart: a provider's per-bucket values under the
 *  active metric. `color` is any CSS color the SVG can paint with —
 *  theme variables included — so the series match the legend. */
export interface AreaSeries {
  key: string;
  label: string;
  color: string;
  /** Stroke/fill opacity multiplier, for series drawn in a neutral tone
   *  that should sit behind the accent colors rather than compete. */
  opacity?: number;
  values: number[];
}

export interface AreaPoint {
  /** Short axis label. */
  label: string;
  /** Full label for the tooltip. */
  subLabel: string;
}

/** Plot margins. No y-axis labels — the tooltip carries the exact
 *  figures, and the hero stat above carries the total — so the plot
 *  can run edge to edge and only the tick row below needs room. */
const PAD_TOP = 10;
const PAD_BOTTOM = 22;
const PAD_X = 4;
const GRIDLINES = 3;
const TICK_LABEL_PX = 10;
/** Minimum horizontal room per axis label before ticks are thinned. */
const TICK_MIN_SPACING_PX = 72;
/** Longest full label that still fits the tick spacing above; "Aug 19"
 *  does, "Yesterday 13:00" does not. */
const TICK_FULL_LABEL_MAX_CHARS = 8;
/** Fallback before the first measurement — wide enough that a server
 *  or jsdom render still lays the points out sensibly. */
const FALLBACK_WIDTH_PX = 640;

/** Smooth `points` with a monotone cubic curve (Fritsch–Carlson
 *  tangents). Unlike a Catmull-Rom spline it never overshoots, so a
 *  run of zeros stays flat on the baseline instead of dipping below
 *  it — a chart of spend must not draw negative money. */
export function monotonePath(points: { x: number; y: number }[]): string {
  const n = points.length;
  if (n === 0) return "";
  if (n === 1) return `M${points[0].x},${points[0].y}`;
  const dx: number[] = [];
  const slope: number[] = [];
  for (let i = 0; i < n - 1; i++) {
    const d = points[i + 1].x - points[i].x;
    dx.push(d);
    slope.push(d === 0 ? 0 : (points[i + 1].y - points[i].y) / d);
  }
  const tangent: number[] = [slope[0]];
  for (let i = 1; i < n - 1; i++) {
    const a = slope[i - 1];
    const b = slope[i];
    tangent.push(a * b <= 0 ? 0 : (a + b) / 2);
  }
  tangent.push(slope[n - 2]);
  // Clamp tangents so the curve stays monotone between samples.
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      tangent[i] = 0;
      tangent[i + 1] = 0;
      continue;
    }
    const a = tangent[i] / slope[i];
    const b = tangent[i + 1] / slope[i];
    const s = a * a + b * b;
    if (s > 9) {
      const t = 3 / Math.sqrt(s);
      tangent[i] = t * a * slope[i];
      tangent[i + 1] = t * b * slope[i];
    }
  }
  let d = `M${fmt(points[0].x)},${fmt(points[0].y)}`;
  for (let i = 0; i < n - 1; i++) {
    const p0 = points[i];
    const p1 = points[i + 1];
    const h = dx[i] / 3;
    d += ` C${fmt(p0.x + h)},${fmt(p0.y + tangent[i] * h)} ${fmt(p1.x - h)},${fmt(
      p1.y - tangent[i + 1] * h,
    )} ${fmt(p1.x)},${fmt(p1.y)}`;
  }
  return d;
}

function fmt(n: number): string {
  return Number.isInteger(n) ? String(n) : n.toFixed(2);
}

/** Evenly spaced tick indices that always include the first and last
 *  bucket, thinned so labels never collide at 24 hours or 90 days. */
export function tickIndices(count: number, width: number): number[] {
  if (count <= 0) return [];
  if (count === 1) return [0];
  const maxTicks = Math.max(2, Math.floor(width / TICK_MIN_SPACING_PX));
  if (count <= maxTicks) return Array.from({ length: count }, (_, i) => i);
  const step = Math.ceil((count - 1) / (maxTicks - 1));
  const ticks: number[] = [];
  for (let i = 0; i < count - 1; i += step) {
    // Drop a tick that would crowd the final one.
    if (count - 1 - i < step / 2) break;
    ticks.push(i);
  }
  ticks.push(count - 1);
  return ticks;
}

function useMeasuredWidth<T extends HTMLElement>(): [
  React.RefObject<T | null>,
  number,
] {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH_PX);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.getBoundingClientRect().width;
      if (w > 0) setWidth(w);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

export function UsageAreaChart({
  series,
  points,
  height,
  hovered,
  onHover,
  formatValue,
  totalLabel = "Total",
  ariaLabel,
  className,
}: {
  series: AreaSeries[];
  points: AreaPoint[];
  height: number;
  hovered: number | null;
  onHover: (index: number | null) => void;
  formatValue: (value: number) => string;
  totalLabel?: string;
  ariaLabel: string;
  className?: string;
}) {
  const [ref, width] = useMeasuredWidth<HTMLDivElement>();
  const gradientId = useId();
  const count = points.length;

  const plotLeft = PAD_X;
  const plotRight = Math.max(plotLeft + 1, width - PAD_X);
  const plotTop = PAD_TOP;
  const plotBottom = Math.max(plotTop + 1, height - PAD_BOTTOM);
  const plotWidth = plotRight - plotLeft;
  const plotHeight = plotBottom - plotTop;

  const max =
    Math.max(0, ...series.flatMap((s) => s.values.map((v) => (Number.isFinite(v) ? v : 0)))) ||
    1;

  const xAt = (i: number) =>
    count <= 1 ? plotLeft + plotWidth / 2 : plotLeft + (i / (count - 1)) * plotWidth;
  const yAt = (v: number) => plotBottom - (Math.max(0, v) / max) * plotHeight;

  const paths = series.map((s) => {
    const pts = Array.from({ length: count }, (_, i) => ({
      x: xAt(i),
      y: yAt(s.values[i] ?? 0),
    }));
    const line = monotonePath(pts);
    const area =
      count >= 2 && line
        ? `${line} L${fmt(pts[count - 1].x)},${fmt(plotBottom)} L${fmt(pts[0].x)},${fmt(
            plotBottom,
          )} Z`
        : "";
    return { series: s, pts, line, area };
  });

  const ticks = tickIndices(count, plotWidth);
  // Prefer the full "Aug 19" over a bare "19" — a day number alone is
  // ambiguous once a 90-day axis spans months. Hourly "Today 13:00"
  // labels are too long and fall back to the compact "13:00" form.
  const fullTicks = ticks.every(
    (i) => (points[i]?.subLabel.length ?? 0) <= TICK_FULL_LABEL_MAX_CHARS,
  );
  const tickLabel = (i: number) =>
    fullTicks ? points[i]?.subLabel : points[i]?.label;

  const indexFromClientX = (clientX: number): number | null => {
    const el = ref.current;
    if (!el || count === 0) return null;
    const rect = el.getBoundingClientRect();
    const x = clientX - rect.left;
    if (count === 1) return 0;
    const i = Math.round(((x - plotLeft) / plotWidth) * (count - 1));
    return Math.min(count - 1, Math.max(0, i));
  };

  const hoveredPoint = hovered === null ? null : (points[hovered] ?? null);
  const hoverX = hovered === null ? null : xAt(hovered);
  const total =
    hovered === null ? 0 : series.reduce((sum, s) => sum + (s.values[hovered] ?? 0), 0);
  // Flip the tooltip to the left of the crosshair past the midpoint so it
  // never runs off the card's right edge.
  const tooltipLeft = hoverX !== null && hoverX > width / 2;

  return (
    <div
      ref={ref}
      className={cn("relative w-full select-none", className)}
      style={{ height }}
      onPointerMove={(e) => onHover(indexFromClientX(e.clientX))}
      onPointerLeave={() => onHover(null)}
    >
      <svg
        width="100%"
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={ariaLabel}
        className="block overflow-visible"
      >
        <defs>
          {series.map((s) => (
            <linearGradient
              key={s.key}
              id={`${gradientId}-${s.key}`}
              x1="0"
              y1="0"
              x2="0"
              y2="1"
            >
              <stop offset="0%" stopColor={s.color} stopOpacity={0.32 * (s.opacity ?? 1)} />
              <stop offset="70%" stopColor={s.color} stopOpacity={0.06 * (s.opacity ?? 1)} />
              <stop offset="100%" stopColor={s.color} stopOpacity={0} />
            </linearGradient>
          ))}
        </defs>

        {/* Gridlines: quiet horizontal guides plus the baseline. */}
        {Array.from({ length: GRIDLINES + 1 }, (_, i) => {
          const y = plotBottom - (i / GRIDLINES) * plotHeight;
          return (
            <line
              key={i}
              x1={plotLeft}
              x2={plotRight}
              y1={y}
              y2={y}
              stroke="var(--border)"
              strokeOpacity={i === 0 ? 1 : 0.5}
              strokeWidth={1}
              strokeDasharray={i === 0 ? undefined : "2 4"}
              shapeRendering="crispEdges"
            />
          );
        })}

        {/* Larger series first so the smaller ones draw on top and stay
            visible where the lines overlap. */}
        {paths.map(({ series: s, area, line }) => (
          <g key={s.key} opacity={s.opacity ?? 1}>
            {area && <path d={area} fill={`url(#${gradientId}-${s.key})`} />}
            {line && (
              <path
                d={line}
                fill="none"
                stroke={s.color}
                strokeWidth={1.75}
                strokeLinejoin="round"
                strokeLinecap="round"
                vectorEffect="non-scaling-stroke"
              />
            )}
          </g>
        ))}

        {/* Crosshair and the points it passes through. */}
        {hoverX !== null && (
          <g>
            <line
              x1={hoverX}
              x2={hoverX}
              y1={plotTop}
              y2={plotBottom}
              stroke="var(--foreground)"
              strokeOpacity={0.35}
              strokeWidth={1}
              strokeDasharray="3 3"
            />
            {paths.map(({ series: s, pts }) => {
              const p = pts[hovered as number];
              if (!p) return null;
              return (
                <g key={s.key} opacity={s.opacity ?? 1}>
                  <circle cx={p.x} cy={p.y} r={6} fill={s.color} fillOpacity={0.2} />
                  <circle
                    cx={p.x}
                    cy={p.y}
                    r={3}
                    fill={s.color}
                    stroke="var(--background)"
                    strokeWidth={1.5}
                  />
                </g>
              );
            })}
          </g>
        )}

        {ticks.map((i) => {
          const x = xAt(i);
          const anchor = i === 0 ? "start" : i === count - 1 ? "end" : "middle";
          return (
            <text
              key={i}
              x={x}
              y={height - 6}
              textAnchor={anchor}
              fontSize={TICK_LABEL_PX}
              className={cn(
                "font-mono uppercase tracking-[0.06em] transition-[fill]",
              )}
              fill={hovered === i ? "var(--foreground)" : "var(--muted-foreground)"}
              fillOpacity={hovered === i ? 1 : 0.75}
            >
              {tickLabel(i)}
            </text>
          );
        })}
      </svg>

      {hoveredPoint && hoverX !== null && (
        <div
          role="tooltip"
          className={cn(
            "pointer-events-none absolute top-1 z-10 min-w-[168px] rounded-md border border-border/70 bg-popover/95 px-3 py-2 shadow-lg backdrop-blur-sm",
          )}
          style={
            tooltipLeft
              ? { right: Math.max(0, width - hoverX + 12) }
              : { left: Math.min(hoverX + 12, Math.max(0, width - 180)) }
          }
        >
          <p className="mb-1.5 font-mono text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground">
            {hoveredPoint.subLabel}
          </p>
          <ul className="flex flex-col gap-1">
            {series.map((s) => (
              <li
                key={s.key}
                className="flex items-center justify-between gap-4 text-[11px]"
              >
                <span className="inline-flex items-center gap-2 text-foreground/85">
                  <span
                    className="h-2 w-2 shrink-0 rounded-[2px]"
                    style={{ background: s.color, opacity: s.opacity ?? 1 }}
                    aria-hidden
                  />
                  {s.label}
                </span>
                <span className="font-mono tabular-nums text-foreground">
                  {formatValue(s.values[hovered as number] ?? 0)}
                </span>
              </li>
            ))}
          </ul>
          {series.length > 1 && (
            <p className="mt-1.5 flex items-center justify-between gap-4 border-t border-border/50 pt-1.5 text-[11px]">
              <span className="text-muted-foreground">{totalLabel}</span>
              <span className="font-mono font-semibold tabular-nums">
                {formatValue(total)}
              </span>
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/** A bare, unlabelled version of the same curve for the provider lanes —
 *  one series, no hover, no axis. */
export function UsageSparkline({
  values,
  color,
  opacity = 1,
  height,
  className,
}: {
  values: number[];
  color: string;
  opacity?: number;
  height: number;
  className?: string;
}) {
  const [ref, width] = useMeasuredWidth<HTMLSpanElement>();
  const gradientId = useId();
  const count = values.length;
  const max = Math.max(0, ...values) || 1;
  const top = 2;
  const bottom = height - 1;
  const pts = values.map((v, i) => ({
    x: count <= 1 ? width / 2 : (i / (count - 1)) * width,
    y: bottom - (Math.max(0, v) / max) * (bottom - top),
  }));
  const line = monotonePath(pts);
  const area =
    count >= 2 ? `${line} L${fmt(width)},${bottom} L0,${bottom} Z` : "";
  return (
    <span
      ref={ref}
      className={cn("block w-full", className)}
      style={{ height }}
      aria-hidden
    >
      <svg
        width="100%"
        height={height}
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        className="block overflow-visible"
        opacity={opacity}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={0.28} />
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
      </svg>
    </span>
  );
}
