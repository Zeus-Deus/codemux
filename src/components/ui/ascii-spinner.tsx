import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";

const SPINNER_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/**
 * Braille spinner used as the leading glyph for a workspace whose agent is
 * actively working. Shared by the expanded sidebar row and the collapsed
 * rail flyout so both animate identically.
 */
export function AsciiSpinner({ className }: { className?: string }) {
  const [frame, setFrame] = useState(0);
  const frameRef = useRef(0);

  useEffect(() => {
    const id = setInterval(() => {
      frameRef.current = (frameRef.current + 1) % SPINNER_FRAMES.length;
      setFrame(frameRef.current);
    }, 80);
    return () => clearInterval(id);
  }, []);

  return (
    <span
      className={cn(
        "text-amber-500 text-sm leading-none select-none",
        className,
      )}
      aria-label="Agent working"
    >
      {SPINNER_FRAMES[frame]}
    </span>
  );
}
