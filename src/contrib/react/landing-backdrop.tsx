/**
 * `landing.backdrop`: plugin contributions mounted behind the new-thread
 * landing with pointer-events: none. The host forwards left clicks on empty
 * landing space (not buttons, links or inputs) to subscribers of `onPulse`
 * and exposes the headline block's rect so a backdrop can dim behind it,
 * with a signal when that rect may have moved (resize or scroll).
 *
 * With no contribution the wrapper is not rendered at all: the landing's DOM
 * is exactly what it was before plugins existed.
 */
import { useCallback, useEffect, useMemo, useRef, type ReactNode } from "react";
import { LANDING_BACKDROP_AREA } from "@/sdk/areas";
import type { LandingBackdropProps } from "@/sdk/host";
import { ContribBoundary, ContribRender } from "./boundary";
import { useContributions } from "./use-contributions";

const INTERACTIVE =
  "button, a, input, textarea, select, label, summary, [role=button], [role=link], [role=menuitem], [role=option], [role=switch], [role=tab], [role=combobox], [contenteditable=''], [contenteditable=true], [data-radix-popper-content-wrapper]";

export function LandingBackdropHost({ children }: { children: ReactNode }) {
  const items = useContributions(LANDING_BACKDROP_AREA);
  const handlers = useRef(new Set<(x: number, y: number) => void>());
  const rootRef = useRef<HTMLDivElement | null>(null);

  const onPulse = useCallback((handler: (x: number, y: number) => void) => {
    handlers.current.add(handler);
    return () => {
      handlers.current.delete(handler);
    };
  }, []);
  const calmRect = useCallback(
    () =>
      rootRef.current?.querySelector<HTMLElement>("[data-backdrop-calm]")?.getBoundingClientRect() ?? null,
    [],
  );
  const calmHandlers = useRef(new Set<() => void>());
  const onCalmChange = useCallback((handler: () => void) => {
    calmHandlers.current.add(handler);
    return () => {
      calmHandlers.current.delete(handler);
    };
  }, []);
  const props = useMemo<LandingBackdropProps>(
    () => ({ onPulse, calmRect, onCalmChange }),
    [onPulse, calmRect, onCalmChange],
  );

  const active = items.length > 0;
  useEffect(() => {
    const root = rootRef.current;
    if (!active || !root) return;
    const notify = () => {
      for (const handler of [...calmHandlers.current]) {
        try {
          handler();
        } catch (error) {
          console.error("[plugins] landing backdrop onCalmChange handler threw", error);
        }
      }
    };
    // The headline moves when the landing resizes, when its column scrolls
    // in a short pane, or when a notice above the composer changes height.
    const ro = new ResizeObserver(notify);
    ro.observe(root);
    const calm = root.querySelector<HTMLElement>("[data-backdrop-calm]");
    if (calm) ro.observe(calm);
    root.addEventListener("scroll", notify, { capture: true, passive: true });
    return () => {
      ro.disconnect();
      root.removeEventListener("scroll", notify, { capture: true });
    };
  }, [active]);

  if (!active) return <>{children}</>;

  return (
    <div
      ref={rootRef}
      className="relative isolate h-full w-full"
      data-landing-backdrop-host
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        const target = event.target as Element | null;
        if (target?.closest(INTERACTIVE)) return;
        for (const handler of [...handlers.current]) {
          try {
            handler(event.clientX, event.clientY);
          } catch (error) {
            console.error("[plugins] landing backdrop onPulse handler threw", error);
          }
        }
      }}
    >
      <div aria-hidden className="pointer-events-none absolute inset-0 -z-10 overflow-hidden">
        {items.map((c) =>
          c.render ? (
            <ContribBoundary id={c.id} key={`${c.source ?? "core"}:${c.id}`} variant="silent">
              <ContribRender render={c.render} props={props} />
            </ContribBoundary>
          ) : null,
        )}
      </div>
      {children}
    </div>
  );
}
