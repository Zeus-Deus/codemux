/** Short wheel glides with continuous velocity when another tick arrives. */
const TICK_DURATION_MS = 200;
const MIN_DURATION_MS = 30;

export interface WheelScrollAdapter {
  read(): number;
  write(position: number): void;
  maximum(): number;
  connected?(): boolean;
}

export interface WheelScrollClock {
  now(): number;
  request(callback: (time: number) => void): number;
  cancel(frame: number): void;
}

/** Accumulate signed input without restarting the glide from zero velocity. */
export function createWheelScrollAnimation(
  adapter: WheelScrollAdapter,
  clock: WheelScrollClock,
) {
  let frame: number | null = null;
  let position = 0;
  let destination = 0;
  let maximum = 0;
  let lastWritten = 0;
  let velocity = 0;
  let startPosition = 0;
  let startVelocity = 0;
  let startTime = 0;
  let duration = TICK_DURATION_MS;
  let remainder = 0;

  const cancel = () => {
    if (frame !== null) clock.cancel(frame);
    frame = null;
    remainder = 0;
  };

  // A cubic Hermite segment starts at the current position/velocity and
  // finishes at the accumulated destination with zero velocity. Retargeting
  // preserves that velocity rather than producing a new stop/start per tick.
  const advance = (time: number) => {
    const t = Math.max(0, Math.min(1, (time - startTime) / duration));
    const distance = destination - startPosition;
    const slope = startVelocity * duration;
    position = startPosition + distance * t * t * (3 - 2 * t) +
      slope * t * (1 - t) * (1 - t);
    velocity = (distance * 6 * t * (1 - t) +
      slope * (1 - 4 * t + 3 * t * t)) / duration;
    return t === 1;
  };

  const tick = (time: number) => {
    frame = null;
    // Layout anchoring, keyboard navigation and following a new message own
    // their position. Never pull the viewport back to an obsolete destination.
    if (adapter.connected?.() === false || Math.abs(adapter.read() - lastWritten) > 1) {
      remainder = 0;
      return;
    }
    maximum = Math.max(0, adapter.maximum());
    destination = Math.max(0, Math.min(maximum, destination));
    const settled = advance(time);
    adapter.write(Math.max(0, Math.min(maximum, position)));
    lastWritten = adapter.read();
    if (settled) {
      // WebKit may quantize scrollTop to whole pixels. Carry the subpixel
      // remainder into the next gesture instead of losing tiny wheel deltas.
      const residual = destination - lastWritten;
      remainder = Math.abs(residual) < 1 ? residual : 0;
    } else frame = clock.request(tick);
  };

  return {
    cancel,
    scrollBy(delta: number): boolean {
      if (!Number.isFinite(delta) || delta === 0) return false;
      const actual = adapter.read();
      const now = clock.now();
      if (frame === null || Math.abs(actual - lastWritten) > 1) {
        const residual = Math.abs(actual - lastWritten) <= 1 ? remainder : 0;
        cancel();
        position = actual;
        lastWritten = actual;
        velocity = 0;
        destination = actual + residual;
        maximum = Math.max(0, adapter.maximum());
      } else advance(now);
      const next = Math.max(0, Math.min(maximum, destination + delta));
      if (next === position && frame === null) return false;
      destination = next;
      const distance = destination - position;
      // Small ticks glide for ~200 ms. Larger accumulated movement and an
      // already moving wheel shorten the segment, so fast input keeps up.
      duration = TICK_DURATION_MS -
        Math.min(360, Math.max(0, Math.abs(distance) - 120)) / 360 * 100;
      if (velocity * distance > 0) {
        duration = Math.min(duration, 2.5 * Math.abs(distance / velocity));
      }
      duration = Math.max(MIN_DURATION_MS, duration);
      startPosition = position;
      // Bound extreme slopes near an edge, while allowing a brief continuous
      // turnaround on reversal as Chromium does. Signed input is never lost.
      startVelocity = Math.sign(velocity) *
        Math.min(Math.abs(velocity), 3 * Math.abs(distance) / duration);
      startTime = now;
      if (frame === null) frame = clock.request(tick);
      return true;
    },
  };
}

const nativeInput = ".xterm, .cm-editor, textarea, input, select, [contenteditable]:not([contenteditable=false]), [data-native-wheel]";
let scrollInstalledElement: ((element: HTMLElement, delta: number) => boolean) | null = null;

/** Used by the transcript navigation rail, which is a sibling of its viewport. */
export function tryAnimatedWheelScroll(element: HTMLElement, delta: number): boolean {
  return scrollInstalledElement?.(element, delta) ?? false;
}

/**
 * Default Linux wheel behavior. DOM wheel events cannot reliably distinguish
 * a high-res mouse from a vertical trackpad gesture. Horizontal input and
 * controls with their own wheel behavior keep their native path.
 */
export function installWheelScrolling(doc: Document = document): () => void {
  const win = doc.defaultView;
  if (!win) return () => {};
  const reducedMotion = win.matchMedia?.("(prefers-reduced-motion: reduce)");
  const clock: WheelScrollClock = {
    now: () => win.performance.now(),
    request: (callback) => win.requestAnimationFrame(callback),
    cancel: (frame) => win.cancelAnimationFrame(frame),
  };
  let active: { element: HTMLElement; animation: ReturnType<typeof createWheelScrollAnimation> } | null = null;
  const cancel = () => {
    active?.animation.cancel();
    active = null;
  };
  const animate = (element: HTMLElement, delta: number) => {
    if (reducedMotion?.matches || !element.isConnected) return false;
    if (active?.element !== element) {
      const maximum = element.scrollHeight - element.clientHeight;
      if (maximum <= 0 || (delta > 0 && element.scrollTop >= maximum) ||
          (delta < 0 && element.scrollTop <= 0)) return false;
      cancel();
      active = {
        element,
        animation: createWheelScrollAnimation({
          read: () => element.scrollTop,
          write: (position) => { element.scrollTop = position; },
          maximum: () => element.scrollHeight - element.clientHeight,
          connected: () => element.isConnected,
        }, clock),
      };
    }
    return active!.animation.scrollBy(delta);
  };
  scrollInstalledElement = animate;

  const onWheel = (event: WheelEvent) => {
    if (!event.isTrusted) return;
    if (event.defaultPrevented || !event.cancelable || event.ctrlKey || event.metaKey ||
        event.shiftKey || event.deltaX !== 0 || reducedMotion?.matches) {
      cancel();
      return;
    }
    const target = event.target;
    const path = event.composedPath();
    // The transcript rail forwards input to a sibling viewport itself.
    if (path.some((node) => node instanceof Element && node.hasAttribute("data-wheel-forwarder"))) return;
    if (!(target instanceof Element) || target.closest(nativeInput) ||
        path.some((node) => node instanceof Element && node.matches(nativeInput))) {
      cancel();
      return;
    }
    for (const node of path) {
      if (!(node instanceof HTMLElement)) continue;
      const style = win.getComputedStyle(node);
      if (style.overflowY !== "auto" && style.overflowY !== "scroll") continue;
      // overflow-x:auto also computes overflow-y:auto. A code block with no
      // vertical overflow must not take ownership from its parent viewport.
      if (node.scrollHeight <= node.clientHeight) continue;
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 :
        event.deltaMode === 2 ? node.clientHeight : 1);
      if (animate(node, delta)) {
        event.preventDefault();
        return;
      }
      if (style.overscrollBehaviorY === "contain" || style.overscrollBehaviorY === "none") {
        cancel();
        return;
      }
    }
  };
  const onVisibility = () => { if (doc.hidden) cancel(); };
  // Picker menus stop bubbling to escape their enclosing scroll lock. Capture
  // still gives them the same glide; native child owners are excluded above.
  doc.addEventListener("wheel", onWheel, { passive: false, capture: true });
  doc.addEventListener("pointerdown", cancel, { passive: true });
  doc.addEventListener("keydown", cancel);
  doc.addEventListener("visibilitychange", onVisibility);
  reducedMotion?.addEventListener("change", cancel);
  return () => {
    cancel();
    if (scrollInstalledElement === animate) scrollInstalledElement = null;
    doc.removeEventListener("wheel", onWheel, true);
    doc.removeEventListener("pointerdown", cancel);
    doc.removeEventListener("keydown", cancel);
    doc.removeEventListener("visibilitychange", onVisibility);
    reducedMotion?.removeEventListener("change", cancel);
  };
}
