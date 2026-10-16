/**
 * `landing.backdrop`: plugin contributions mounted behind the new-thread
 * landing with pointer-events: none. The host forwards left clicks on empty
 * landing space (not buttons, links or inputs) to subscribers of `onPulse`
 * and exposes the headline block's rect so a backdrop can dim behind it.
 *
 * With no contribution the wrapper is not rendered at all: the landing's DOM
 * is exactly what it was before plugins existed.
 */
import { useCallback, useMemo, useRef, type ReactNode } from "react";
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
  const props = useMemo<LandingBackdropProps>(() => ({ onPulse, calmRect }), [onPulse, calmRect]);

  if (items.length === 0) return <>{children}</>;

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
