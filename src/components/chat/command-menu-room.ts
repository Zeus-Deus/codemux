import { useLayoutEffect, useState, type RefObject } from "react";

/** Space kept free between a composer menu and its anchor (the menus'
 *  `mb-2`) plus the same again above it, so it never touches the edge. */
const MENU_MARGINS_PX = 16;

/** Below this a menu is too short to use; let it clip instead. */
const MIN_ROOM_PX = 120;

/**
 * Height available to a menu that rises from `bottom-full` of its anchor
 * (the composer card) before a clipping ancestor — the chat's scroll area
 * under the header — or the visible viewport cuts it off. `undefined`
 * without layout (jsdom), so callers keep their CSS caps.
 */
export function roomAbove(menu: HTMLElement): number | undefined {
  const anchor = menu.offsetParent ?? menu.parentElement;
  if (!anchor) return undefined;
  const rect = anchor.getBoundingClientRect();
  if (rect.height === 0) return undefined;
  let clipTop = Math.max(0, window.visualViewport?.offsetTop ?? 0);
  for (let el = anchor.parentElement; el; el = el.parentElement) {
    if (getComputedStyle(el).overflowY !== "visible") {
      clipTop = Math.max(clipTop, el.getBoundingClientRect().top);
    }
  }
  return Math.max(MIN_ROOM_PX, Math.floor(rect.top - clipTop - MENU_MARGINS_PX));
}

/** {@link roomAbove}, kept current while the menu is open: the card grows
 *  as the draft wraps, and a phone keyboard resizes the visible viewport. */
export function useRoomAbove(
  ref: RefObject<HTMLElement | null>,
  open: boolean,
): number | undefined {
  const [room, setRoom] = useState<number | undefined>(undefined);
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!open || !menu) return;
    const update = () => setRoom(roomAbove(menu));
    update();
    const viewport = window.visualViewport;
    window.addEventListener("resize", update);
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    const anchor = menu.offsetParent ?? menu.parentElement;
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    if (anchor) observer?.observe(anchor);
    return () => {
      window.removeEventListener("resize", update);
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      observer?.disconnect();
    };
  }, [ref, open]);
  return open ? room : undefined;
}
