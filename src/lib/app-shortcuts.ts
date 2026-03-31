/**
 * App-level keyboard shortcuts that should be intercepted before reaching
 * xterm.js or other focused components. When the terminal has focus, these
 * keys are blocked from the terminal and bubble up to window-level handlers.
 *
 * Dynamically derived from the keybind registry + user overrides.
 */

import { resolveKeybinds } from "@/hooks/use-resolved-keybinds";
import { parseKeyCombo } from "@/lib/keybind-utils";
import { KEYBIND_REGISTRY } from "@/lib/keybind-registry";

export interface AppShortcut {
  key: string; // event.key (lowercase)
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

let _cached: AppShortcut[] = [];
let _lastOverrides: Record<string, string> | null = null;

/** Rebuild the app shortcut list from current overrides. Called by use-keyboard-shortcuts. */
export function updateAppShortcuts(overrides: Record<string, string>) {
  if (overrides === _lastOverrides) return;
  _lastOverrides = overrides;

  const resolved = resolveKeybinds(overrides);
  const shortcuts: AppShortcut[] = [];

  for (const [id, entry] of resolved.keybindMap) {
    // Terminal-level shortcuts are handled inside xterm, not intercepted
    const reg = KEYBIND_REGISTRY.find((e) => e.id === id);
    if (reg?.when === "terminal") continue;
    if (!entry.activeKeys) continue;

    const parsed = parseKeyCombo(entry.activeKeys);
    shortcuts.push({
      key: parsed.key.toLowerCase(),
      ctrl: parsed.ctrl,
      shift: parsed.shift,
      alt: parsed.alt,
    });
  }

  _cached = shortcuts;
}

// Initialize with defaults on module load
updateAppShortcuts({});

/**
 * Check if a keyboard event matches any app-level shortcut.
 * Used by xterm's attachCustomKeyEventHandler to decide whether
 * to let the event bubble (return false) or keep it (return true).
 */
export function isAppShortcut(event: KeyboardEvent): boolean {
  const key = event.key.toLowerCase();
  return _cached.some(
    (s) =>
      s.key === key &&
      s.ctrl === event.ctrlKey &&
      s.shift === event.shiftKey &&
      s.alt === event.altKey,
  );
}
