import { useEffect, useSyncExternalStore } from "react";

// Only browser clients adapt: resizing a native desktop window preserves its UI.
export function mobileLayoutAvailable(): boolean {
  return (
    typeof window !== "undefined" &&
    !(
      "__TAURI_INTERNALS__" in window &&
      !(window as Window & { __CODEMUX_REMOTE__?: boolean })
        .__CODEMUX_REMOTE__ &&
      document.documentElement.dataset.browserClient !== "true"
    )
  );
}
const query = "(max-width: 1023px)";
function subscribe(callback: () => void) {
  if (typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(query);
  media.addEventListener("change", callback);
  return () => media.removeEventListener("change", callback);
}
export function useMobileLayout(): boolean {
  return useSyncExternalStore(
    subscribe,
    () =>
      mobileLayoutAvailable() &&
      typeof window.matchMedia === "function" &&
      window.matchMedia(query).matches,
    () => false,
  );
}

/** Follow the keyboard's visible viewport without disabling accessibility zoom. */
export function useMobileViewport() {
  const mobile = useMobileLayout();
  useEffect(() => {
    if (!mobile) return;
    const root = document.documentElement;
    root.dataset.mobile = "true";
    const viewport = window.visualViewport;
    const update = () => {
      // Pinch zoom must remain native; don't resize/reflow the application while zooming.
      if (viewport && Math.abs(viewport.scale - 1) > 0.01) return;
      const height = viewport?.height ?? window.innerHeight;
      root.style.setProperty("--mobile-height", `${height}px`);
      root.style.setProperty("--mobile-top", `${viewport?.offsetTop ?? 0}px`);
      root.dataset.keyboard =
        window.innerHeight - height > 120 ? "true" : "false";
    };
    update();
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    return () => {
      delete root.dataset.mobile;
      delete root.dataset.keyboard;
      root.style.removeProperty("--mobile-height");
      root.style.removeProperty("--mobile-top");
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [mobile]);
}
