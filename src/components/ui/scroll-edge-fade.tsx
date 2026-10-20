import { useLayoutEffect, useState, type ComponentProps, type RefObject } from "react";

import { cn } from "@/lib/utils";

/** Which edges of a scroller have more content past them. Pass a ref whose
 *  element mounts with the caller, or the element itself (kept in state via
 *  a callback ref) when it can mount, unmount or be replaced later, e.g.
 *  inside a popover that renders its content a commit after opening. */
export function useScrollEdges(target: RefObject<HTMLElement | null> | HTMLElement | null, enabled = true) {
  const [edges, setEdges] = useState({ top: false, bottom: false });

  useLayoutEffect(() => {
    const node = target && "current" in target ? target.current : target;
    if (!enabled || !node) return;

    const sync = () => {
      const top = node.scrollTop > 3;
      const bottom = node.scrollHeight - node.clientHeight - node.scrollTop > 3;
      setEdges((current) =>
        current.top === top && current.bottom === bottom ? current : { top, bottom },
      );
    };
    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(sync);
    const observeContent = () => {
      resizeObserver?.disconnect();
      resizeObserver?.observe(node);
      if (node.firstElementChild) resizeObserver?.observe(node.firstElementChild);
      sync();
    };
    observeContent();

    // Filtering can change scrollHeight without resizing the viewport itself.
    const mutationObserver = new MutationObserver(observeContent);
    mutationObserver.observe(node, {
      attributes: true,
      attributeFilter: ["hidden"],
      childList: true,
      characterData: true,
      subtree: true,
    });
    node.addEventListener("scroll", sync, { passive: true });
    window.addEventListener("resize", sync);
    return () => {
      resizeObserver?.disconnect();
      mutationObserver.disconnect();
      node.removeEventListener("scroll", sync);
      window.removeEventListener("resize", sync);
    };
  }, [target, enabled]);

  return edges;
}

/** Place at an edge of a positioned scroll shell; keep scroll-padding at least
 *  as tall as the overlay so keyboard selection stays clear of it. */
export function ScrollEdgeFade({
  edge,
  visible,
  surface = "var(--popover)",
  className,
  style,
  ...props
}: ComponentProps<"div"> & {
  edge: "top" | "bottom";
  visible: boolean;
  surface?: string;
}) {
  return (
    <div
      {...props}
      aria-hidden="true"
      data-scroll-edge={edge}
      data-visible={visible}
      className={cn(
        "pointer-events-none absolute inset-x-0 z-10 h-5 transition-opacity duration-150 motion-reduce:transition-none",
        edge === "top" ? "top-0" : "bottom-0",
        visible ? "opacity-100" : "opacity-0",
        className,
      )}
      // Small gradient overlays avoid the CPU renderer's scroll-mask cost.
      style={{
        backgroundImage: `linear-gradient(to ${edge === "top" ? "bottom" : "top"}, ${surface}, transparent)`,
        ...style,
      }}
    />
  );
}
