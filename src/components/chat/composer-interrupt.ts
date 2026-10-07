import { formatKeyCombo } from "@/components/ui/menu-chrome";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { normalizeKeyCombo } from "@/lib/keybind-utils";

/** The Stop button's tooltip, naming the key that does the same, with Escape
 *  shortened to the "Esc" printed on the keycap. */
export function useStopTooltip(): string {
  const keys = useResolvedKeybinds().getKeysForAction("interruptAgent");
  if (!keys) return "Stop";
  return `Stop (${formatKeyCombo(keys).replace(/\bEscape\b/, "Esc")})`;
}

/**
 * Whether a keydown in the chat composer asks to stop the running agent: the
 * user's `interruptAgent` binding (Escape by default), or Ctrl+C while the
 * composer is empty and nothing is selected, the terminal habit for "stop".
 * Unbinding `interruptAgent` in Settings turns both off.
 */
export function isInterruptKey(
  event: KeyboardEvent,
  boundKeys: string,
  draft: string,
): boolean {
  if (event.repeat || !boundKeys) return false;
  const combo = normalizeKeyCombo(event);
  if (!combo) return false;
  if (combo === boundKeys) return true;
  return (
    combo === "Ctrl+C" &&
    draft.length === 0 &&
    !window.getSelection()?.toString()
  );
}

/**
 * Stop the agent once the key has finished travelling, unless something else
 * claimed it first. Escape is the lowest-priority claim in the window: closing
 * the composer strip, leaving a subagent drill-in or dismissing an overlay all
 * mark the event handled, and none of them should also kill the run.
 */
export function interruptUnlessClaimed(event: KeyboardEvent, onStop: () => void): void {
  setTimeout(() => {
    if (!event.defaultPrevented) onStop();
  }, 0);
}
