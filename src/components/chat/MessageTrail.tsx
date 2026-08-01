import type * as React from "react";
import type { LegendListRef } from "@legendapp/list/react";
import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { cn } from "@/lib/utils";
import { normalizeWheelDelta } from "@/lib/wheel";

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
/** Desired viewport-relative offset of a jumped-to turn's top. */
const SCROLL_MARGIN_PX = 10;
/** Estimated preview-card height, used only to clamp it within the rail. */
const CARD_EST_H = 96;

/**
 * Message **navigation trail** for the Agent Chat transcript: a slim gutter
 * of tick marks in the transcript's left margin, one per user turn. Hover a
 * tick to preview the turn (prompt + reply start); click to jump to it. The
 * tick for the turn currently in view is highlighted.
 *
 * Rendered as a sibling of LegendList. Active-row tracking and jumps go
 * through the list's index API, so the rail does not assume its target is
 * currently mounted.
 *
 * Memoized: it re-derives its entries from every slot, so it must not be
 * dragged through a render triggered by something it does not consume.
 */
export const MessageTrail = memo(function MessageTrail({
  slots,
  listRef,
  firstVisibleSlotIndex,
}: {
  slots: TranscriptSlot[];
  listRef: React.RefObject<LegendListRef | null>;
  firstVisibleSlotIndex: number;
}) {
  const entries = useMemo(() => buildTrailEntries(slots), [slots]);

  if (entries.length < TRAIL_MIN_TURNS) return null;
  return (
    <TrailRail
      entries={entries}
      listRef={listRef}
      firstVisibleSlotIndex={firstVisibleSlotIndex}
    />
  );
});

const TrailRail = memo(function TrailRail({
  entries,
  listRef,
  firstVisibleSlotIndex,
}: {
  entries: TrailEntry[];
  listRef: React.RefObject<LegendListRef | null>;
  firstVisibleSlotIndex: number;
}) {
  const railRef = useRef<HTMLElement | null>(null);
  const [railHeight, setRailHeight] = useState(0);
  const [hovered, setHovered] = useState<{
    entryIndex: number;
    centerY: number;
  } | null>(null);
  const showTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const activeIndex = useMemo(
    () => deriveActiveTrailIndex(entries, firstVisibleSlotIndex),
    [entries, firstVisibleSlotIndex],
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

  // The rail overlays the list but is its sibling, so wheel input over the
  // rail must be forwarded to LegendList's scroll node.
  const forwardWheel = useCallback(
    (e: React.WheelEvent) => {
      const viewport = listRef.current?.getScrollableNode();
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
      viewport.scrollTop += normalizeWheelDelta(e.deltaY, e.deltaMode);
    },
    [listRef],
  );

  const jumpTo = useCallback(
    (slotIndex: number) => {
      void listRef.current?.scrollToIndex({
        index: slotIndex,
        animated: false,
        viewOffset: SCROLL_MARGIN_PX,
      });
    },
    [listRef],
  );

  const hoveredEntry = hovered ? entries[hovered.entryIndex] : null;

  return (
    <nav
      ref={railRef}
      aria-label="Conversation turns"
      onWheel={forwardWheel}
      onMouseLeave={hidePreview}
      onBlur={hidePreview}
      className="group/trail absolute inset-y-0 left-0 z-20 flex w-7 flex-col items-center justify-center gap-[1.5px] opacity-55 transition-opacity duration-300 group-hover/transcript-list:opacity-100"
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
            onClick={() => jumpTo(entry.slotIndex)}
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
});

/** First `n` words of a prompt, for the tick's aria-label. */
function firstWords(text: string, n = 6): string {
  const words = text.split(/\s+/).filter(Boolean).slice(0, n);
  return words.length ? words.join(" ") : "empty turn";
}

/** Keep the preview card within the rail's vertical bounds. */
function clampCardTop(centerY: number, railHeight: number): number {
  const half = CARD_EST_H / 2;
  const min = half + 6;
  const max = railHeight - half - 6;
  if (max <= min) return centerY; // rail too short to clamp meaningfully
  return Math.min(Math.max(centerY, min), max);
}
