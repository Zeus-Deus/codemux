/**
 * Kitty keyboard protocol utilities.
 *
 * Implements level 1 ("disambiguate") of the Kitty keyboard protocol:
 * https://sw.kovidgoyal.net/kitty/keyboard-protocol/
 *
 * When a CLI app pushes Kitty mode, modified special keys (Shift+Enter,
 * Ctrl+Tab, etc.) are encoded as CSI u sequences instead of their
 * ambiguous legacy encodings.
 */

/**
 * Functional keys that get CSI u encoding when modified.
 * Escape is intentionally omitted — handled by the interrupt handler.
 * Arrow/Home/End/etc are omitted — xterm.js already encodes them with modifiers.
 *
 * Enter, Tab, and Space are encoded unconditionally (like Ghostty, Kitty, WezTerm).
 * Backspace is only encoded when Kitty mode is active (kittyLevel > 0), so that
 * the backward-kill-word keybind (Ctrl+Backspace → \x17) works in plain shells.
 */
export const KITTY_FUNCTIONAL_KEYS: ReadonlyMap<string, number> = new Map([
  ["Enter", 13],
  ["Tab", 9],
  ["Backspace", 127],
  [" ", 32],
]);

/** Codepoint for Backspace — gated behind kittyLevel > 0 (see key handler). */
export const BACKSPACE_CODEPOINT = 127;

/** CSI u modifier param: 1 + shift + 2*alt + 4*ctrl + 8*super */
export function csiUModifier(ev: {
  shiftKey: boolean;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
}): number {
  return (
    1 +
    (ev.shiftKey ? 1 : 0) +
    (ev.altKey ? 2 : 0) +
    (ev.ctrlKey ? 4 : 0) +
    (ev.metaKey ? 8 : 0)
  );
}

/** Format a CSI u sequence */
export function csiUSequence(codepoint: number, mod: number): string {
  return `\x1b[${codepoint};${mod}u`;
}

/** Result of scanning a PTY output chunk for Kitty protocol sequences */
export interface KittyScanResult {
  hasQuery: boolean;
  pushValues: number[];
  popCount: number;
  hasDAQuery: boolean;
}

/** Scan PTY output for Kitty keyboard protocol sequences and DA queries */
export function scanKittySequences(str: string): KittyScanResult {
  const hasQuery = /\x1b\[\?u/.test(str);

  const pushValues: number[] = [];
  for (const m of str.matchAll(/\x1b\[>([0-9]+)u/g)) {
    pushValues.push(parseInt(m[1], 10));
  }

  const popCount = (str.match(/\x1b\[<u/g) ?? []).length;

  // DA query: shell sends \x1b[c (DA1) or \x1b[>c (DA2) on startup.
  // Either form may include an explicit 0 param (\x1b[0c / \x1b[>0c).
  // Indicates a new shell started — Kitty state should be reset.
  const hasDAQuery = /\x1b\[>?0?c/.test(str);

  return { hasQuery, pushValues, popCount, hasDAQuery };
}

/** Apply push/pop/reset operations to a Kitty flags stack */
export function applyKittyStack(
  stack: readonly number[],
  pushValues: readonly number[],
  popCount: number,
  resetAll: boolean,
): number[] {
  const newStack = resetAll ? [] : [...stack];
  for (const val of pushValues) {
    newStack.push(val);
  }
  for (let i = 0; i < popCount && newStack.length > 0; i++) {
    newStack.pop();
  }
  return newStack;
}

/** Get current Kitty flags (top of stack, or 0) */
export function kittyFlags(stack: readonly number[]): number {
  return stack.length > 0 ? stack[stack.length - 1] : 0;
}
