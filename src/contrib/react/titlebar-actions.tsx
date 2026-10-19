import { useLayoutEffect, useRef, type CSSProperties } from "react";
import { create } from "zustand";
import { TITLEBAR_ACTIONS_AREA } from "@/sdk/areas";
import { cn } from "@/lib/utils";
import { Slot } from "./slot";
import { useContributions } from "./use-contributions";

const useActionWidth = create(() => ({ width: 0 }));
/** Extra room needed by the shared plugin action island, including its gap. */
export function useTitlebarActionsWidth() { return useActionWidth((s) => s.width); }

/** The same slot in regular, covered and standalone window chrome. */
export function TitlebarActions({ className, style }: { className?: string; style?: CSSProperties }) {
  const items = useContributions(TITLEBAR_ACTIONS_AREA);
  const visible = items.some((c) => c.render);
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = ref.current;
    if (!node) { useActionWidth.setState({ width: 0 }); return; }
    const measure = () => {
      const width = node.getBoundingClientRect().width;
      useActionWidth.setState({ width: width > 0 ? Math.ceil(width) + 4 : 0 });
    };
    measure();
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(node);
    return () => { observer?.disconnect(); useActionWidth.setState({ width: 0 }); };
  }, [items]);
  if (!visible) return null;
  return <div ref={ref} data-plugin-titlebar-actions
    className={cn("pointer-events-auto mr-1 flex shrink-0 items-center gap-1", className)} style={style}
    onPointerDown={(e) => e.stopPropagation()}>
    <Slot area={TITLEBAR_ACTIONS_AREA} />
  </div>;
}
