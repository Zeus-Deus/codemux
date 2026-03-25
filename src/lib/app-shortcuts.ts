/**
 * App-level keyboard shortcuts that should be intercepted before reaching
 * xterm.js or other focused components. When the terminal has focus, these
 * keys are blocked from the terminal and bubble up to window-level handlers.
 *
 * Keep in sync with: use-keyboard-shortcuts.ts, app-shell.tsx (Ctrl+K)
 */

export interface AppShortcut {
  key: string;    // event.key (lowercase)
  ctrl: boolean;
  shift: boolean;
}

export const APP_SHORTCUTS: AppShortcut[] = [
  // Command palette
  { key: "k", ctrl: true, shift: false },
  // File search
  { key: "p", ctrl: true, shift: false },
  // Content search
  { key: "f", ctrl: true, shift: true },
  // Split pane right
  { key: "d", ctrl: true, shift: true },
  // Close pane
  { key: "w", ctrl: true, shift: true },
  // New tab
  { key: "t", ctrl: true, shift: false },
  // Close tab
  { key: "w", ctrl: true, shift: false },
  // Toggle sidebar
  { key: "b", ctrl: true, shift: false },
  // Cycle workspace forward/backward
  { key: "]", ctrl: true, shift: false },
  { key: "[", ctrl: true, shift: false },
  // Tab switching (Ctrl+1 through Ctrl+9)
  { key: "1", ctrl: true, shift: false },
  { key: "2", ctrl: true, shift: false },
  { key: "3", ctrl: true, shift: false },
  { key: "4", ctrl: true, shift: false },
  { key: "5", ctrl: true, shift: false },
  { key: "6", ctrl: true, shift: false },
  { key: "7", ctrl: true, shift: false },
  { key: "8", ctrl: true, shift: false },
  { key: "9", ctrl: true, shift: false },
];

/**
 * Check if a keyboard event matches any app-level shortcut.
 * Used by xterm's attachCustomKeyEventHandler to decide whether
 * to let the event bubble (return false) or keep it (return true).
 */
export function isAppShortcut(event: KeyboardEvent): boolean {
  if (!event.ctrlKey) return false;
  const key = event.key.toLowerCase();
  return APP_SHORTCUTS.some(
    (s) => s.key === key && s.ctrl === event.ctrlKey && s.shift === event.shiftKey,
  );
}
