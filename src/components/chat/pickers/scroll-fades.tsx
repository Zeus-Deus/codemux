import { useLayoutEffect, useState } from "react";
import { cn } from "@/lib/utils";

export interface ScrollEdges {
  /** There is more to scroll above. */
  top: boolean;
  /** There is more to scroll below. */
  bottom: boolean;
}

/**
 * Which edges of a scroller have content past them. Follows scrolling,
 * resizing and rows coming and going. Attach the returned callback as the
 * scroller's `ref`.
 */
export function useScrollEdges<T extends HTMLElement>(): [(node: T | null) => void, ScrollEdges] {
  const [node, setNode] = useState<T | null>(null);
  const [edges, setEdges] = useState<ScrollEdges>({ top: false, bottom: false });
  useLayoutEffect(() => {
    if (!node) return;
    const sync = () => {
      const top = node.scrollTop > 1;
      const bottom = node.scrollHeight - node.clientHeight - node.scrollTop > 1;
      setEdges((current) => (current.top === top && current.bottom === bottom ? current : { top, bottom }));
    };
    sync();
    node.addEventListener("scroll", sync, { passive: true });
    // The scroller's own box is fixed; its content is what grows, as rows
    // come and go or a refreshed label wraps.
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(sync);
    const observeContent = () => {
      resize?.observe(node);
      for (const child of node.children) resize?.observe(child);
    };
    observeContent();
    const mutation = new MutationObserver(() => {
      observeContent();
      sync();
    });
    mutation.observe(node, { childList: true, subtree: true, characterData: true });
    return () => {
      node.removeEventListener("scroll", sync);
      resize?.disconnect();
      mutation.disconnect();
    };
  }, [node]);
  return [setNode, edges];
}

/**
 * Gradient fades over the edges of a scroller that have more past them.
 * Gradients rather than a mask, so rows under the fade keep their own
 * colors. Render as siblings of the scroller inside a `relative` box;
 * `className` sets the gradient's color (`from-…`) and horizontal extent.
 */
export function EdgeFades({
  edges,
  className,
  testId,
}: {
  edges: ScrollEdges;
  className: string;
  testId: string;
}) {
  const fade =
    "pointer-events-none absolute left-0 right-0 z-10 h-6 to-transparent transition-opacity duration-150 ease-out motion-reduce:transition-none";
  return (
    <>
      <div
        aria-hidden
        data-testid={`${testId}-top-fade`}
        data-visible={edges.top}
        className={cn(fade, "top-0 bg-gradient-to-b", className, edges.top ? "opacity-100" : "opacity-0")}
      />
      <div
        aria-hidden
        data-testid={`${testId}-bottom-fade`}
        data-visible={edges.bottom}
        className={cn(fade, "bottom-0 bg-gradient-to-t", className, edges.bottom ? "opacity-100" : "opacity-0")}
      />
    </>
  );
}
