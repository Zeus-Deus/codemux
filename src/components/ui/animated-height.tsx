import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Slightly past the `base` duration: releases the clip when no
 *  `transitionend` arrives (reduced motion, an interrupted resize). */
const CLIP_FALLBACK_MS = 250;

/**
 * Eases its own height whenever its content resizes: a row joining a list, a
 * section expanding, one body swapped for a taller one. The first measurement
 * lands without a transition, so restored state never animates in.
 *
 * Content is clipped only while a resize is in flight, so focus rings and
 * shadows are free to overhang at rest. Keep it off streaming content: a
 * height that changes every few frames would chase its own tail and drag
 * the scroll anchor with it.
 */
export function AnimatedHeight({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  const contentRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<{ height: number | null; clipping: boolean }>({
    height: null,
    clipping: false,
  });

  useLayoutEffect(() => {
    const element = contentRef.current;
    if (!element) return;
    const measure = () => {
      const height = element.offsetHeight;
      setState((current) =>
        current.height === height
          ? current
          : { height, clipping: current.height !== null },
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!state.clipping) return;
    const timer = window.setTimeout(
      () => setState((current) => ({ ...current, clipping: false })),
      CLIP_FALLBACK_MS,
    );
    return () => window.clearTimeout(timer);
  }, [state.height, state.clipping]);

  return (
    <div
      data-slot="animated-height"
      className={cn(
        "transition-[height] duration-150 ease-drawer motion-reduce:transition-none",
        className,
      )}
      style={
        state.height === null
          ? undefined
          : { height: state.height, overflow: state.clipping ? "clip" : undefined }
      }
      onTransitionEnd={(event) => {
        if (event.target !== event.currentTarget || event.propertyName !== "height") return;
        setState((current) => (current.clipping ? { ...current, clipping: false } : current));
      }}
    >
      {/* `flow-root` keeps a child's margins inside the measured box. */}
      <div ref={contentRef} className="flow-root">
        {children}
      </div>
    </div>
  );
}
