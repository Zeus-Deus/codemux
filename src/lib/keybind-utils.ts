/**
 * Pure utility functions for keyboard shortcut normalization and matching.
 * No React or store dependencies.
 */

const MODIFIER_KEYS = new Set([
  "Control", "Shift", "Alt", "Meta",
  "CapsLock", "NumLock", "ScrollLock",
]);

/** Canonical modifier order: Alt → Ctrl → Shift */
export function normalizeKeyCombo(event: KeyboardEvent): string {
  if (MODIFIER_KEYS.has(event.key)) return "";

  const parts: string[] = [];
  if (event.altKey) parts.push("Alt");
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.shiftKey) parts.push("Shift");

  parts.push(normalizeKeyName(event.key));
  return parts.join("+");
}

/** Normalize a raw event.key value to canonical display form */
function normalizeKeyName(key: string): string {
  // Space is event.key " " (length 1) but should display as "Space"
  if (key === " ") return "Space";
  // Single alphabetic character → uppercase
  if (key.length === 1 && /[a-zA-Z]/.test(key)) return key.toUpperCase();
  // Single digit
  if (key.length === 1 && /[0-9]/.test(key)) return key;
  // Punctuation (brackets, etc.) — keep as-is
  if (key.length === 1) return key;

  // Named keys — normalize casing
  const named: Record<string, string> = {
    Escape: "Escape",
    Enter: "Enter",
    Tab: "Tab",
    Backspace: "Backspace",
    Delete: "Delete",
    Space: "Space",
    " ": "Space",
    ArrowUp: "ArrowUp",
    ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft",
    ArrowRight: "ArrowRight",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
  };

  // Add F-keys
  for (let i = 1; i <= 12; i++) named[`F${i}`] = `F${i}`;

  return named[key] ?? key;
}

export interface ParsedKeyCombo {
  alt: boolean;
  ctrl: boolean;
  shift: boolean;
  key: string;
}

/** Parse a canonical combo string like "Ctrl+Shift+D" into its parts */
export function parseKeyCombo(combo: string): ParsedKeyCombo {
  const parts = combo.split("+");
  const key = parts[parts.length - 1];
  return {
    alt: parts.includes("Alt"),
    ctrl: parts.includes("Ctrl"),
    shift: parts.includes("Shift"),
    key,
  };
}

/** Check if a keyboard event matches a canonical combo string */
export function matchesKeyCombo(event: KeyboardEvent, combo: string): boolean {
  if (!combo) return false;
  const parsed = parseKeyCombo(combo);
  if (event.altKey !== parsed.alt) return false;
  if (event.ctrlKey !== parsed.ctrl) return false;
  if (event.shiftKey !== parsed.shift) return false;
  return normalizeKeyName(event.key) === parsed.key;
}

/** Returns true if the event is only a modifier key press (no action key) */
export function isModifierOnly(event: KeyboardEvent): boolean {
  return MODIFIER_KEYS.has(event.key);
}
