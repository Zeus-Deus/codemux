import { useState } from "react";
import { WindowChrome } from "@/components/layout/window-chrome";
import wordmarkSource from "@/assets/codemux-wordmark.svg?raw";

/** Tailwind's `animate-pulse` cycle, in milliseconds. */
const PULSE_MS = 2000;

/**
 * The wordmark's glyphs with their hard-coded fill swapped for
 * `currentColor`. An `<img>` cannot take a theme colour, so the splash inlines
 * the paths and lets `text-foreground` paint them, the same way the static
 * `#splash` in `index.html` paints them with `--cm-boot-fg`.
 */
const WORDMARK_GLYPHS = wordmarkSource
  .replace(/^[\s\S]*?<svg[^>]*>|<\/svg>\s*$/g, "")
  .replace(/fill="[^"]*"/g, 'fill="currentColor"');

/**
 * The one loading screen for app start: auth check, then app state and
 * settings, then the first Home draft. Every stage renders this same
 * component, so the hand-offs between them are invisible instead of three
 * different layouts cutting into each other.
 *
 * The wordmark uses the static `#splash`'s box and viewBox (`index.html`), so
 * the crossfade from that splash to this one does not move or resize it.
 *
 * Each stage mounts its own copy, which would restart the pulse. A negative
 * delay pinned to the page clock keeps every copy on the same phase, so the
 * wordmark never jumps back to full opacity at a hand-off.
 *
 * The phase is read once per mount. The shell re-renders the same splash as
 * app state, settings and flags arrive, and a delay that changed on each of
 * those renders would be applied retroactively by the browser, making the
 * pulse jump exactly at the hand-offs it is meant to smooth.
 */
export function BootSplash() {
  const [phase] = useState(() =>
    typeof performance === "undefined" ? 0 : performance.now() % PULSE_MS,
  );
  return (
    <div
      className="relative flex h-screen w-screen items-center justify-center bg-background"
      role="status"
      aria-label="Loading Codemux"
    >
      <WindowChrome />
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width="320"
        height="69"
        viewBox="78 18 204 44"
        fill="none"
        aria-hidden="true"
        data-testid="boot-wordmark"
        className="shrink-0 select-none text-foreground opacity-80 motion-safe:animate-pulse"
        style={{ animationDelay: `-${Math.round(phase)}ms` }}
        dangerouslySetInnerHTML={{ __html: WORDMARK_GLYPHS }}
      />
    </div>
  );
}
