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
  findTrailEntryAtOffset,
  sampleTrailIndices,
  type TrailEntry,
} from "./message-trail";
import type { TranscriptSlot } from "./transcript-slots";

/** Below this many user turns the trail is pure noise — hide it entirely
 *  (short threads scroll fine on their own). */
const TRAIL_MIN_TURNS = 3;
/** Hard ceiling on rendered ticks; a very long thread downsamples to this
 *  (see `sampleTrailIndices`). */
const TRAIL_MAX_TICKS = 60;
/** Approx vertical space one tick (bar + hit-area padding + gap) occupies;
 *  the effective tick cap is derived from the measured rail height so the
 *  gutter never overflows on a short viewport. */
const TICK_SLOT_PX = 10;
/** Top/bottom breathing room reserved inside the rail. */
const RAIL_PAD_PX = 12;
/** Desired viewport-relative offset of a jumped-to turn's top. */
const SCROLL_MARGIN_PX = 10;
/** Estimated preview-card height, used only to clamp it within the rail. */
const CARD_EST_H = 96;

/**
 * Message **navigation trail** for the Agent Chat transcript: a slim gutter
 * of tick marks in the transcript's left margin, one per user turn. Hover a
 * tick to preview the turn (prompt + reply start); click to jump to it. The
 * ticks for turns currently in view are highlighted.
 *
 * Rendered as a sibling of LegendList. Visibility tracking reads the list's
 * position model and updates tick attributes directly on animation frames;
 * scrolling therefore never schedules a React render of the transcript.
 *
 * Memoized: it re-derives its entries from every slot, so it must not be
 * dragged through a render triggered by something it does not consume.
 */
export const MessageTrail = memo(function MessageTrail({
  slots,
  listRef,
}: {
  slots: TranscriptSlot[];
  listRef: React.RefObject<LegendListRef | null>;
}) {
  const entries = useMemo(() => buildTrailEntries(slots), [slots]);

  if (entries.length < TRAIL_MIN_TURNS) return null;
  return (
    <TrailRail entries={entries} listRef={listRef} />
  );
});

const TrailRail = memo(function TrailRail({
  entries,
  listRef,
}: {
  entries: TrailEntry[];
  listRef: React.RefObject<LegendListRef | null>;
}) {
  const railRef = useRef<HTMLElement | null>(null);
  const tickRefs = useRef(new Map<number, HTMLSpanElement>());
  const [railHeight, setRailHeight] = useState(0);
  const [hovered, setHovered] = useState<{
    entryIndex: number;
    centerY: number;
  } | null>(null);

  // Effective tick cap fits the measured rail so the gutter never overflows.
  const maxTicks = useMemo(() => {
    const avail = railHeight - RAIL_PAD_PX * 2;
    if (avail <= 0) return TRAIL_MAX_TICKS; // pre-measure: assume it fits
    return Math.max(
      TRAIL_MIN_TURNS,
      Math.min(TRAIL_MAX_TICKS, Math.floor(avail / TICK_SLOT_PX)),
    );
  }, [railHeight]);

  const ticks = useMemo(
    () => sampleTrailIndices(entries.length, maxTicks),
    [entries.length, maxTicks],
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

  // LegendList already owns the hot scroll path. Reading its position model
  // once per animation frame and mutating these tiny presentation attributes
  // avoids the previous onFirstVisibleItemChanged -> setState -> React commit
  // cycle for every visible-row transition. This is the standard
  // editor-minimap strategy and works even when target rows are virtualized
  // away.
  useEffect(() => {
    const viewport = listRef.current?.getScrollableNode();
    if (!viewport) return;

    let frame = 0;
    const sync = () => {
      frame = 0;
      const state = listRef.current?.getState();
      if (!state) return;

      const scrollTop = viewport.scrollTop;
      const scrollBottom = scrollTop + viewport.clientHeight;
      const activeEntryIndex = findTrailEntryAtOffset(
        entries,
        scrollTop + 1,
        state.positionAtIndex,
      );

      let currentTick = -1;
      let nearestDistance = Infinity;
      for (const entryIndex of ticks) {
        const distance = Math.abs(entryIndex - activeEntryIndex);
        if (activeEntryIndex >= 0 && distance < nearestDistance) {
          nearestDistance = distance;
          currentTick = entryIndex;
        }
      }

      for (const entryIndex of ticks) {
        const strip = tickRefs.current.get(entryIndex);
        if (!strip) continue;
        const entry = entries[entryIndex];
        const rowTop = state.positionAtIndex(entry.slotIndex);
        const rowHeight = state.sizeAtIndex(entry.slotIndex);
        const inView =
          Number.isFinite(rowTop) &&
          rowTop < scrollBottom &&
          rowTop + Math.max(1, Number.isFinite(rowHeight) ? rowHeight : 1) >
            scrollTop;
        const current = entryIndex === currentTick;
        const nextInView = String(inView);
        const nextCurrent = String(current);
        if (strip.dataset.inView !== nextInView) {
          strip.dataset.inView = nextInView;
        }
        if (strip.dataset.current !== nextCurrent) {
          strip.dataset.current = nextCurrent;
        }
        const button = strip.parentElement;
        if (current && button?.getAttribute("aria-current") !== "true") {
          button?.setAttribute("aria-current", "true");
        } else if (!current && button?.hasAttribute("aria-current")) {
          button.removeAttribute("aria-current");
        }
      }
    };
    const scheduleSync = () => {
      if (frame === 0) frame = requestAnimationFrame(sync);
    };

    scheduleSync();
    viewport.addEventListener("scroll", scheduleSync, { passive: true });
    // A viewport resize (window resize, or a LegendList re-measurement that
    // shifts rows without moving `scrollTop`) changes what is visible without
    // firing `scroll`, which would otherwise leave the ticks stale until the
    // next scroll. Route it through the same rAF-coalesced sync.
    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(scheduleSync);
    resizeObserver?.observe(viewport);
    return () => {
      viewport.removeEventListener("scroll", scheduleSync);
      resizeObserver?.disconnect();
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  }, [entries, listRef, ticks]);

  const showPreview = useCallback(
    (entryIndex: number, target: HTMLElement) => {
      const rail = railRef.current;
      if (!rail) return;
      const railRect = rail.getBoundingClientRect();
      const r = target.getBoundingClientRect();
      const centerY = r.top - railRect.top + r.height / 2;
      setHovered({ entryIndex, centerY });
    },
    [],
  );

  const hidePreview = useCallback(() => {
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
  const hoveredTickPosition = hovered
    ? ticks.indexOf(hovered.entryIndex)
    : -1;

  return (
    <nav
      ref={railRef}
      aria-label="Conversation turns"
      onWheel={forwardWheel}
      onMouseLeave={hidePreview}
      onBlur={hidePreview}
      className="absolute inset-y-0 left-0 z-20 flex w-7 flex-col items-center justify-center gap-[1.5px] opacity-55 transition-opacity duration-150 group-hover/transcript-list:opacity-100"
    >
      {ticks.map((entryIndex, tickPosition) => {
        const entry = entries[entryIndex];
        const hoverDistance =
          hoveredTickPosition < 0
            ? null
            : Math.abs(tickPosition - hoveredTickPosition);
        return (
          <button
            key={entry.messageId}
            type="button"
            aria-label={`Jump to turn ${entry.turnIndex + 1}: ${firstWords(entry.userText)}`}
            onClick={() => jumpTo(entry.slotIndex)}
            onMouseEnter={(e) => showPreview(entryIndex, e.currentTarget)}
            onFocus={(e) => showPreview(entryIndex, e.currentTarget)}
            className="group/tick flex w-full items-center justify-center py-[3px] outline-none"
          >
            <span
              ref={(node) => {
                if (node) tickRefs.current.set(entryIndex, node);
                else tickRefs.current.delete(entryIndex);
              }}
              data-current="false"
              data-in-view="false"
              className={cn(
                "h-0.5 rounded-full bg-muted-foreground/30 transition-[width,background-color] duration-150 data-[current=true]:bg-foreground/70 data-[in-view=true]:bg-foreground/90",
                hoverDistance === 0
                  ? "w-6 bg-muted-foreground/75"
                  : hoverDistance === 1
                    ? "w-4"
                    : hoverDistance === 2
                      ? "w-2.5"
                      : "w-2",
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
