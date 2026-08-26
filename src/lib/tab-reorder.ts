/**
 * Pointer-based drag-to-reorder for a horizontal strip of tabs. Shared by
 * the titlebar's workspace tabs and the right panel's pane deck, so the two
 * strips have one drag feel: the same movement threshold before a press
 * turns into a drag, the same drop-indicator geometry, the same
 * click-suppression after a drop.
 *
 * HTML5 DnD is avoided on purpose: both strips sit in the window's titlebar
 * band next to a live `data-tauri-drag-region`, and stacking native HTML5
 * drag — which Tauri/WebKit's own drag-region handling already interposes
 * on — on top of that is exactly the "not clean" combination the GUI-chrome
 * doc flagged. Pointer events don't interact with the OS drag region at
 * all and give full control over when a "drag" actually starts.
 *
 * Listeners for pointermove/up/cancel are attached to `document` (not the
 * pill itself) for the lifetime of one pointer session, so fast pointer
 * movement that outruns the small pill's bounds — which would otherwise
 * stop delivering events to a per-element listener — still gets tracked.
 * `setPointerCapture` isn't used: capturing on the pointerdown target would
 * retarget the browser's synthesized `click` to that same target too,
 * which would break the inner activate/close buttons ever receiving a
 * plain click.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";

/** Movement (px) before a pointerdown on a pill turns into a reorder drag
 *  instead of resolving as a plain click. Small enough to feel immediate,
 *  large enough that a normal click/tap never misfires as a drag. */
export const DRAG_THRESHOLD = 5;

export interface PillReorderHandlers {
  "data-tab-id": string;
  onPointerDown: (e: React.PointerEvent<HTMLElement>) => void;
  onClickCapture: (e: React.MouseEvent<HTMLElement>) => void;
}

export interface TabReorder<T extends HTMLElement> {
  /** Place on the scrolling strip element — the drop index is measured
   *  against its `[data-tab-id]` descendants. */
  containerRef: React.MutableRefObject<T | null>;
  /** Id of the tab being dragged, once the threshold is crossed. */
  dragTabId: string | null;
  /** Drop-indicator x in the strip's *content* coordinate space (already
   *  offset by `scrollLeft`), or null while nothing is being dragged. */
  dropIndicatorLeft: number | null;
  /** Spread onto each tab's outer element. Controls inside a tab that must
   *  keep a plain click (close buttons, chevrons) opt out with
   *  `data-no-drag`. */
  getPillProps: (tabId: string) => PillReorderHandlers;
}

/**
 * @param ids The strip's current order. Read at drop time, so the caller
 *   never has to worry about the list changing mid-drag.
 * @param onReorder Called with the full new order once a drag actually
 *   moved a tab; never called for a no-op drop or a plain click.
 */
export function useTabReorder<T extends HTMLElement = HTMLDivElement>(
  ids: readonly string[],
  onReorder: (ids: string[]) => void,
): TabReorder<T> {
  const containerRef = useRef<T | null>(null);
  const [dragTabId, setDragTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndexState] = useState<number | null>(null);

  // Refs mirror state the pointer listeners need to read/write without
  // forcing a hook re-subscription or a render on every write that
  // doesn't need one.
  const dropIndexRef = useRef<number | null>(null);
  const idsRef = useRef(ids);
  idsRef.current = ids;
  const onReorderRef = useRef(onReorder);
  onReorderRef.current = onReorder;

  const pendingTabIdRef = useRef<string | null>(null);
  const pointerIdRef = useRef<number | null>(null);
  const startPosRef = useRef({ x: 0, y: 0 });
  const draggingRef = useRef(false);
  const suppressClickRef = useRef(false);
  const cleanupRef = useRef<(() => void) | null>(null);

  const setDropIndex = useCallback((v: number | null) => {
    dropIndexRef.current = v;
    setDropIndexState(v);
  }, []);

  const computeDropIndex = useCallback(
    (clientX: number) => {
      const el = containerRef.current;
      if (!el) return;
      const tabEls = el.querySelectorAll<HTMLElement>("[data-tab-id]");
      if (tabEls.length === 0) return;

      let closestIdx = 0;
      let closestDist = Infinity;
      let insertBefore = true;
      tabEls.forEach((tabEl, i) => {
        const rect = tabEl.getBoundingClientRect();
        const midX = rect.left + rect.width / 2;
        const dist = Math.abs(clientX - midX);
        if (dist < closestDist) {
          closestDist = dist;
          closestIdx = i;
          insertBefore = clientX < midX;
        }
      });
      setDropIndex(insertBefore ? closestIdx : closestIdx + 1);
    },
    [setDropIndex],
  );

  const endSession = useCallback(
    (commit: boolean) => {
      cleanupRef.current?.();
      cleanupRef.current = null;

      const tabId = pendingTabIdRef.current;
      const wasDragging = draggingRef.current;
      const finalDropIndex = dropIndexRef.current;

      pendingTabIdRef.current = null;
      pointerIdRef.current = null;
      draggingRef.current = false;
      setDragTabId(null);
      setDropIndex(null);

      if (commit && wasDragging && tabId != null && finalDropIndex != null) {
        const currentIds = idsRef.current;
        const dragIdx = currentIds.indexOf(tabId);
        if (dragIdx >= 0) {
          const newIds = [...currentIds];
          newIds.splice(dragIdx, 1);
          const insertAt =
            finalDropIndex > dragIdx ? finalDropIndex - 1 : finalDropIndex;
          newIds.splice(Math.min(insertAt, newIds.length), 0, tabId);
          if (newIds.join(",") !== currentIds.join(",")) {
            onReorderRef.current(newIds);
          }
        }
      }
    },
    [setDropIndex],
  );

  const handlePointerDown = useCallback(
    (tabId: string) => (e: React.PointerEvent<HTMLElement>) => {
      // Only the primary button/contact starts a reorder session, never one
      // already in flight, and never from a close/chevron control
      // explicitly opted out via `data-no-drag` — those need their plain
      // click behavior preserved untouched.
      if (e.button !== 0) return;
      if ((e.target as HTMLElement).closest("[data-no-drag]")) return;
      if (pendingTabIdRef.current != null) return;

      pendingTabIdRef.current = tabId;
      pointerIdRef.current = e.pointerId;
      startPosRef.current = { x: e.clientX, y: e.clientY };
      draggingRef.current = false;

      const handleMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        const dx = ev.clientX - startPosRef.current.x;
        const dy = ev.clientY - startPosRef.current.y;
        if (!draggingRef.current) {
          if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
          draggingRef.current = true;
          setDragTabId(pendingTabIdRef.current);
        }
        ev.preventDefault();
        computeDropIndex(ev.clientX);
      };

      const handleUp = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        if (draggingRef.current) suppressClickRef.current = true;
        endSession(true);
      };

      const handleCancel = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        endSession(false);
      };

      document.addEventListener("pointermove", handleMove);
      document.addEventListener("pointerup", handleUp);
      document.addEventListener("pointercancel", handleCancel);
      cleanupRef.current = () => {
        document.removeEventListener("pointermove", handleMove);
        document.removeEventListener("pointerup", handleUp);
        document.removeEventListener("pointercancel", handleCancel);
      };
    },
    [computeDropIndex, endSession],
  );

  // Belt-and-suspenders: drop any live document listeners if the strip
  // unmounts mid-drag (e.g. workspace switch).
  useEffect(() => () => cleanupRef.current?.(), []);

  const getPillProps = useCallback(
    (tabId: string): PillReorderHandlers => ({
      "data-tab-id": tabId,
      onPointerDown: handlePointerDown(tabId),
      onClickCapture: (e: React.MouseEvent<HTMLElement>) => {
        // Swallow the click synthesized after a real drag so it doesn't
        // activate the tab (or, for the active chat tab, pop the history
        // dropdown) as a side effect of the reorder gesture.
        if (suppressClickRef.current) {
          e.preventDefault();
          e.stopPropagation();
          suppressClickRef.current = false;
        }
      },
    }),
    [handlePointerDown],
  );

  // Drop-indicator position, converted into the strip's scrolled content
  // coordinate space (`+ scrollLeft`) since the strip scrolls horizontally.
  let dropIndicatorLeft: number | null = null;
  if (dragTabId && dropIndex !== null && containerRef.current) {
    const el = containerRef.current;
    const tabEls = el.querySelectorAll<HTMLElement>("[data-tab-id]");
    const listRect = el.getBoundingClientRect();
    if (tabEls.length > 0) {
      if (dropIndex >= tabEls.length) {
        const lastRect = tabEls[tabEls.length - 1].getBoundingClientRect();
        dropIndicatorLeft = lastRect.right - listRect.left + el.scrollLeft;
      } else {
        const targetRect = tabEls[dropIndex].getBoundingClientRect();
        dropIndicatorLeft = targetRect.left - listRect.left + el.scrollLeft;
      }
    }
  }

  return { containerRef, dragTabId, dropIndicatorLeft, getPillProps };
}
