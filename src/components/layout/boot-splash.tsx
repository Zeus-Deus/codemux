import { WindowChrome } from "@/components/layout/window-chrome";
import wordmark from "@/assets/codemux-wordmark.svg";

/** Tailwind's `animate-pulse` cycle, in milliseconds. */
const PULSE_MS = 2000;

/**
 * The one loading screen for app start: auth check, then app state and
 * settings, then the first Home draft. Every stage renders this same
 * component, so the hand-offs between them are invisible instead of three
 * different layouts cutting into each other.
 *
 * Each stage mounts its own copy, which would restart the pulse. A negative
 * delay pinned to the page clock keeps every copy on the same phase, so the
 * wordmark never jumps back to full opacity at a hand-off.
 */
export function BootSplash() {
  const phase =
    typeof performance === "undefined" ? 0 : performance.now() % PULSE_MS;
  return (
    <div
      className="relative flex h-screen w-screen items-center justify-center bg-background"
      role="status"
      aria-label="Loading Codemux"
    >
      <WindowChrome />
      <img
        src={wordmark}
        alt=""
        draggable={false}
        className="h-12 w-auto select-none opacity-80 motion-safe:animate-pulse"
        style={{ animationDelay: `-${Math.round(phase)}ms` }}
      />
    </div>
  );
}
