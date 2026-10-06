import { useEffect, useState } from "react";
import { create } from "zustand";
import { TYPOGRAPHY_DEFAULTS, TYPOGRAPHY_RANGES } from "@/lib/typography";
import { cn } from "@/lib/utils";

/** How long the pill holds after the last press before it fades. */
const HOLD_MS = 900;

interface InterfaceSizeAnnouncement {
  size: number;
  /** The press asked to go past the range and the size stayed put. */
  atLimit: "min" | "max" | null;
  /** Bumped on every press, so a repeat of the same size still restarts the hold. */
  seq: number;
}

const useInterfaceSizeHudStore = create<{ announcement: InterfaceSizeAnnouncement | null }>(
  () => ({ announcement: null }),
);

/**
 * Report an interface-size shortcut. Called for every press, including one
 * that hits the end of the range and changes nothing — that is exactly the
 * press that otherwise looks like a dead key.
 */
export function announceInterfaceSize(size: number, step: "in" | "out" | "reset", changed: boolean): void {
  const { min, max } = TYPOGRAPHY_RANGES.interface;
  const atLimit =
    changed || step === "reset"
      ? null
      : step === "in" && size >= max
        ? "max"
        : step === "out" && size <= min
          ? "min"
          : null;
  useInterfaceSizeHudStore.setState((state) => ({
    announcement: { size, atLimit, seq: (state.announcement?.seq ?? 0) + 1 },
  }));
}

export function interfaceSizeLabel(announcement: Pick<InterfaceSizeAnnouncement, "size" | "atLimit">): string {
  const { size, atLimit } = announcement;
  if (atLimit === "max") return `Maximum size · ${size}px`;
  if (atLimit === "min") return `Minimum size · ${size}px`;
  return size === TYPOGRAPHY_DEFAULTS.interfaceSize ? `Interface ${size}px · default` : `Interface ${size}px`;
}

/**
 * A small transient pill confirming Ctrl+= / Ctrl+- / Ctrl+0. One instance,
 * not a toast, so held or repeated presses update it in place instead of
 * stacking notifications.
 */
export function InterfaceSizeHud() {
  const announcement = useInterfaceSizeHudStore((s) => s.announcement);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (!announcement) return;
    setVisible(true);
    const timer = window.setTimeout(() => setVisible(false), HOLD_MS);
    return () => window.clearTimeout(timer);
  }, [announcement]);

  if (!announcement) return null;
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="interface-size-hud"
      data-visible={visible}
      className={cn(
        "pointer-events-none fixed top-14 left-1/2 z-[60] flex h-7 min-w-[140px] -translate-x-1/2 items-center justify-center rounded-md border border-hairline-strong bg-popover px-3 font-mono text-label text-popover-foreground tabular-nums shadow-md transition-opacity duration-150 motion-reduce:transition-none",
        visible ? "opacity-100" : "opacity-0",
      )}
    >
      {interfaceSizeLabel(announcement)}
    </div>
  );
}
