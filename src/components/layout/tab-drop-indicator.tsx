/**
 * The drop cue for a tab strip mid-drag: a vertical mirror of the sidebar's
 * leading-dot + thin neutral line. No accent color, so it reads as a UI
 * cue rather than an alert. Positioned in the strip's content coordinate
 * space (see `useTabReorder`'s `dropIndicatorLeft`), so the host strip must
 * be `relative`.
 */
export function TabDropIndicator({ left }: { left: number }) {
  return (
    <div
      className="pointer-events-none absolute inset-y-0.5 z-30 flex flex-col items-center"
      style={{ left: left - 1, width: 2 }}
      data-testid="tab-drop-indicator"
    >
      <div className="-mt-0.5 h-1.5 w-1.5 shrink-0 rounded-full bg-foreground/70" />
      <div className="w-px flex-1 rounded-full bg-foreground/40" />
    </div>
  );
}
