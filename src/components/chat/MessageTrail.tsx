import type * as React from "react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  useMessageScroller,
  useMessageScrollerVisibility,
} from "@/components/ui/message-scroller";
import { cn } from "@/lib/utils";

import {
  buildTrailEntries,
  deriveActiveTrailIndex,
  sampleTrailIndices,
  withActiveIndex,
  type TrailEntry,
} from "./message-trail";
import type { TranscriptSlot } from "./transcript-slots";

/** Below this many user turns the trail is pure noise — hide it entirely
 *  (short threads scroll fine on their own). */
const TRAIL_MIN_TURNS = 3;
/** Hard ceiling on rendered ticks; a very long thread downsamples to this
 *  (see `sampleTrailIndices`). The active turn is always re-injected. */
const TRAIL_MAX_TICKS = 60;
/** Approx vertical space one tick (bar + hit-area padding + gap) occupies;
 *  the effective tick cap is derived from the measured rail height so the
 *  gutter never overflows on a short viewport. */
const TICK_SLOT_PX = 10;
/** Top/bottom breathing room reserved inside the rail. */
const RAIL_PAD_PX = 12;
/** Small delay before a hover preview shows, so scrubbing the rail doesn't
 *  flicker a card at every tick. */
const SHOW_DELAY_MS = 120;
/** Desired viewport-relative offset of the jumped-to turn's top (a hair
 *  below the viewport's top fade edge). Both the engine jump's `scrollMargin`
 *  and the corrective settle loop below aim for this. */
const SCROLL_MARGIN_PX = 10;
/** The target is "settled" once its measured top is within this many px of
 *  `SCROLL_MARGIN_PX`. */
const SETTLE_TOLERANCE_PX = 2;
/** Consecutive in-tolerance frames required before the settle loop stops. A
 *  cold jump crosses rows whose `content-visibility:auto` sizes are still
 *  finalizing, so a single in-tolerance frame isn't proof it will hold —
 *  we wait for a few stable frames. */
const SETTLE_STABLE_FRAMES = 3;
/** Frame budget for the settle loop (~0.6s at 60fps). `content-visibility`
 *  rows only expose ESTIMATED heights until rendered, so re-calling
 *  `scrollToMessage` just reproduces the same wrong offset; instead we nudge
 *  scrollTop by the target row's measured REAL error each frame and
 *  re-measure until it holds (rows keep their remembered
 *  `contain-intrinsic-size: auto` heights once laid out). */
const SETTLE_MAX_FRAMES = 40;
/** Estimated preview-card height, used only to clamp it within the rail. */
const CARD_EST_H = 96;

/**
 * Message **navigation trail** for the Agent Chat transcript: a slim gutter
 * of tick marks in the transcript's left margin, one per user turn. Hover a
 * tick to preview the turn (prompt + reply start); click to jump to it. The
 * tick for the turn currently in view is highlighted.
 *
 * Rendered inside `MessageList`'s `<MessageScroller>` (inside the provider,
 * a sibling of the viewport). It reads the active turn from the scroller's
 * `visibleMessageIds` — NOT `currentAnchorId`, which is always `null` here
 * because the transcript keeps `scrollAnchor={false}` on every row. Adds no
 * setting: it is part of the (already Beta-gated) pane and simply hides on
 * threads shorter than `TRAIL_MIN_TURNS`.
 */
export function MessageTrail({ slots }: { slots: TranscriptSlot[] }) {
  const entries = useMemo(() => buildTrailEntries(slots), [slots]);
  const slotIndexById = useMemo(() => {
    const map = new Map<string, number>();
    for (let i = 0; i < slots.length; i++) map.set(slots[i].messageId, i);
    return map;
  }, [slots]);

  // Gate the subscribing rail behind the threshold so short threads pay
  // nothing for visibility tracking (the hooks are pay-for-use).
  if (entries.length < TRAIL_MIN_TURNS) return null;
  return <TrailRail entries={entries} slotIndexById={slotIndexById} />;
}

function TrailRail({
  entries,
  slotIndexById,
}: {
  entries: TrailEntry[];
  slotIndexById: Map<string, number>;
}) {
  const { visibleMessageIds } = useMessageScrollerVisibility();
  const { scrollToMessage } = useMessageScroller();

  const railRef = useRef<HTMLElement | null>(null);
  const viewportRef = useRef<HTMLElement | null>(null);
  const [railHeight, setRailHeight] = useState(0);
  const [hovered, setHovered] = useState<{
    entryIndex: number;
    centerY: number;
  } | null>(null);
  const showTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const frameRef = useRef(0);

  const activeIndex = useMemo(
    () => deriveActiveTrailIndex(entries, visibleMessageIds, slotIndexById),
    [entries, visibleMessageIds, slotIndexById],
  );

  // Effective tick cap fits the measured rail so the gutter never overflows.
  const maxTicks = useMemo(() => {
    const avail = railHeight - RAIL_PAD_PX * 2;
    if (avail <= 0) return TRAIL_MAX_TICKS; // pre-measure: assume it fits
    return Math.max(
      TRAIL_MIN_TURNS,
      Math.min(TRAIL_MAX_TICKS, Math.floor(avail / TICK_SLOT_PX)),
    );
  }, [railHeight]);

  const baseSample = useMemo(
    () => sampleTrailIndices(entries.length, maxTicks),
    [entries.length, maxTicks],
  );
  const ticks = useMemo(
    () => withActiveIndex(baseSample, activeIndex),
    [baseSample, activeIndex],
  );

  // Measure the rail (pre-paint) and track resizes so `maxTicks` stays right.
  useLayoutEffect(() => {
    const el = railRef.current;
    if (!el) return;
    const update = () => setRailHeight(el.clientHeight);
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(
    () => () => {
      if (showTimerRef.current) clearTimeout(showTimerRef.current);
      if (frameRef.current) cancelAnimationFrame(frameRef.current);
    },
    [],
  );

  const showPreview = useCallback(
    (entryIndex: number, target: HTMLElement, immediate: boolean) => {
      const rail = railRef.current;
      if (!rail) return;
      const railRect = rail.getBoundingClientRect();
      const r = target.getBoundingClientRect();
      const centerY = r.top - railRect.top + r.height / 2;
      if (showTimerRef.current) clearTimeout(showTimerRef.current);
      if (immediate) {
        setHovered({ entryIndex, centerY });
        return;
      }
      showTimerRef.current = setTimeout(() => {
        setHovered({ entryIndex, centerY });
      }, SHOW_DELAY_MS);
    },
    [],
  );

  const hidePreview = useCallback(() => {
    if (showTimerRef.current) clearTimeout(showTimerRef.current);
    setHovered(null);
  }, []);

  // Resolve the scroll viewport (a SIBLING of the rail under the shared
  // `message-scroller` root) and cache it — used by both the wheel forwarder
  // and the jump settle loop.
  const resolveViewport = useCallback((): HTMLElement | null => {
    const cached = viewportRef.current;
    if (cached?.isConnected) return cached;
    const root = railRef.current?.closest('[data-slot="message-scroller"]');
    const viewport =
      (root as HTMLElement | null)?.querySelector<HTMLElement>(
        '[data-slot="message-scroller-viewport"]',
      ) ?? null;
    viewportRef.current = viewport;
    return viewport;
  }, []);

  // The rail overlays the viewport's left gutter but is its SIBLING, not an
  // ancestor, so a wheel over this strip can't bubble to the viewport
  // natively — forward it manually so the gutter scrolls like the rest of
  // the surface. Two steps, both required:
  //
  // 1. Dispatch a real `wheel` event on the viewport. The scroller engine's
  //    stick-to-bottom state machine only releases its bottom pin on genuine
  //    user input (`wheel`/`touchmove`/nav keys) observed on the viewport; a
  //    bare `scrollTop` write fires only a `scroll` event, which the engine
  //    treats as programmatic — leaving it pinned and snapping the view back
  //    to the tail on the next content mutation mid-stream. (Re-entry is not
  //    a concern: the rail is a sibling of the viewport, so the dispatched
  //    event never bubbles back through this handler.)
  // 2. Mutate `scrollTop` to perform the actual scroll — a synthetic wheel
  //    event carries no default scroll action of its own.
  const forwardWheel = useCallback(
    (e: React.WheelEvent) => {
      const viewport = resolveViewport();
      if (!viewport) return;
      viewport.dispatchEvent(
        new WheelEvent("wheel", {
          deltaY: e.deltaY,
          deltaX: e.deltaX,
          deltaMode: e.deltaMode,
          bubbles: true,
          cancelable: true,
        }),
      );
      viewport.scrollTop += e.deltaY;
    },
    [resolveViewport],
  );

  const jumpTo = useCallback(
    (messageId: string) => {
      // Rough jump via the engine: it resolves the row from its registry and
      // sizes the bottom spacer so the target offset is reachable. But its
      // model uses `content-visibility` ESTIMATED heights, so a cold jump can
      // land the target hundreds of px off — and re-calling it just
      // reproduces the same wrong offset (stable-but-wrong). So we follow up
      // with a REAL-geometry correction loop.
      scrollToMessage(messageId, {
        align: "start",
        behavior: "auto",
        scrollMargin: SCROLL_MARGIN_PX,
      });

      const viewport = resolveViewport();
      if (!viewport) return;

      if (frameRef.current) cancelAnimationFrame(frameRef.current);
      let row = findRow(viewport, messageId);
      let frames = 0;
      let stableFrames = 0;
      const settle = () => {
        if (!row?.isConnected) row = findRow(viewport, messageId);
        if (!row) return;
        // Nudge scrollTop by the target row's MEASURED offset error (the row
        // is now laid out around the viewport, so its geometry is real), and
        // re-measure next frame. Keep going until it holds near
        // SCROLL_MARGIN_PX for a few consecutive frames — a cold jump crosses
        // rows still finalizing their content-visibility sizes, so one
        // in-tolerance frame isn't proof it will stay put.
        const top =
          row.getBoundingClientRect().top -
          viewport.getBoundingClientRect().top;
        const delta = top - SCROLL_MARGIN_PX;
        if (Math.abs(delta) > SETTLE_TOLERANCE_PX) {
          viewport.scrollTop += delta;
          stableFrames = 0;
        } else {
          stableFrames += 1;
        }
        frames += 1;
        if (stableFrames < SETTLE_STABLE_FRAMES && frames < SETTLE_MAX_FRAMES) {
          frameRef.current = requestAnimationFrame(settle);
        }
      };
      frameRef.current = requestAnimationFrame(settle);
    },
    [scrollToMessage, resolveViewport],
  );

  const hoveredEntry = hovered ? entries[hovered.entryIndex] : null;

  return (
    <nav
      ref={railRef}
      aria-label="Conversation turns"
      onWheel={forwardWheel}
      onMouseLeave={hidePreview}
      onBlur={hidePreview}
      className="group/trail absolute inset-y-0 left-0 z-20 flex w-7 flex-col items-center justify-center gap-[1.5px] opacity-55 transition-opacity duration-300 group-hover/message-scroller:opacity-100"
    >
      {ticks.map((entryIndex) => {
        const entry = entries[entryIndex];
        const isActive = entryIndex === activeIndex;
        return (
          <button
            key={entry.messageId}
            type="button"
            aria-label={`Jump to turn ${entry.turnIndex + 1}: ${firstWords(entry.userText)}`}
            aria-current={isActive ? "true" : undefined}
            onClick={() => jumpTo(entry.messageId)}
            onMouseEnter={(e) => showPreview(entryIndex, e.currentTarget, false)}
            onFocus={(e) => showPreview(entryIndex, e.currentTarget, true)}
            className="group/tick flex w-full items-center justify-center py-[3px] outline-none"
          >
            <span
              className={cn(
                "rounded-full transition-all duration-200",
                isActive
                  ? "h-[3px] w-4 bg-foreground/70"
                  : "h-[2px] w-2.5 bg-muted-foreground/30 group-hover/trail:w-3 group-hover/trail:bg-muted-foreground/55 group-focus-visible/tick:h-[3px] group-focus-visible/tick:w-4 group-focus-visible/tick:bg-foreground/60",
              )}
            />
          </button>
        );
      })}

      {hoveredEntry ? (
        <div
          aria-hidden
          style={{ top: clampCardTop(hovered!.centerY, railHeight) }}
          className="pointer-events-none absolute left-full top-0 ml-1.5 w-[260px] -translate-y-1/2 animate-in fade-in-0 rounded-lg border border-border bg-popover p-2.5 text-xs shadow-md duration-150"
        >
          <p className="line-clamp-2 font-medium leading-snug text-foreground">
            {hoveredEntry.userText || "Empty turn"}
          </p>
          {hoveredEntry.replySnippet ? (
            <p className="mt-1 line-clamp-3 leading-snug text-muted-foreground">
              {hoveredEntry.replySnippet}
            </p>
          ) : (
            <p className="mt-1 italic leading-snug text-muted-foreground/70">
              No reply yet
            </p>
          )}
        </div>
      ) : null}
    </nav>
  );
}

/** First `n` words of a prompt, for the tick's aria-label. */
function firstWords(text: string, n = 6): string {
  const words = text.split(/\s+/).filter(Boolean).slice(0, n);
  return words.length ? words.join(" ") : "empty turn";
}

/** Locate a mounted row by its message id without CSS-selector escaping
 *  (message ids include `run:<id>` group keys). */
function findRow(viewport: HTMLElement, messageId: string): HTMLElement | null {
  const rows = viewport.querySelectorAll<HTMLElement>("[data-message-id]");
  for (const row of rows) {
    if (row.dataset.messageId === messageId) return row;
  }
  return null;
}

/** Keep the preview card within the rail's vertical bounds. */
function clampCardTop(centerY: number, railHeight: number): number {
  const half = CARD_EST_H / 2;
  const min = half + 6;
  const max = railHeight - half - 6;
  if (max <= min) return centerY; // rail too short to clamp meaningfully
  return Math.min(Math.max(centerY, min), max);
}
