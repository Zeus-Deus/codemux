/**
 * Diagnostics-only observation of a virtual transcript. Hydration completion
 * and DOM text presence are not visual readiness: LegendList hides its rows
 * until measurement and initial scrolling finish. Require a nonempty row in
 * the viewport, visible ancestors, and another animation frame with the same
 * conditions. This bounds a paint opportunity, not compositor presentation;
 * use screenshots/video as the independent visual check.
 *
 * No store writes, scrolling or observers survive completion/unmount. Callers
 * gate this on tracing so normal navigation pays no DOM polling cost.
 */
export function observeTranscriptReady(
  root: HTMLElement,
  onReady: () => void,
): () => void {
  const deadline = Date.now() + 10_000;
  let visibleLastFrame = false;
  let frame = 0;
  const check = () => {
    if (!root.isConnected || Date.now() >= deadline) return;
    const viewport = root.querySelector<HTMLElement>('[data-slot="transcript-list"]');
    const box = viewport?.getBoundingClientRect();
    let visible = false;
    if (viewport && box && box.width > 0 && box.height > 0) {
      for (const row of viewport.querySelectorAll<HTMLElement>("[data-index]")) {
        if (!row.textContent?.trim()) continue;
        const rect = row.getBoundingClientRect();
        if (
          rect.width <= 0 || rect.height <= 0 ||
          rect.bottom <= Math.max(0, box.top) ||
          rect.top >= Math.min(window.innerHeight, box.bottom) ||
          rect.right <= Math.max(0, box.left) ||
          rect.left >= Math.min(window.innerWidth, box.right)
        ) continue;
        visible = true;
        for (let element: HTMLElement | null = row; element; element = element.parentElement) {
          const style = getComputedStyle(element);
          if (
            style.opacity === "0" || style.display === "none" ||
            style.visibility === "hidden" || style.visibility === "collapse"
          ) {
            visible = false;
            break;
          }
        }
        if (visible) break;
      }
    }
    if (visible && visibleLastFrame) {
      onReady();
      return;
    }
    visibleLastFrame = visible;
    frame = requestAnimationFrame(check);
  };
  frame = requestAnimationFrame(check);
  return () => cancelAnimationFrame(frame);
}
