import type { Terminal } from "@xterm/xterm";

/**
 * Update xterm's renderer in place. Returning false lets callers skip the
 * expensive fit/PTY resize path when a settings event repeats unchanged.
 */
export function applyTerminalTypography(
  terminal: Pick<Terminal, "options" | "clearTextureAtlas">,
  family: string,
  size: number,
): boolean {
  const familyChanged = terminal.options.fontFamily !== family;
  const sizeChanged = terminal.options.fontSize !== size;
  if (!familyChanged && !sizeChanged) return false;
  terminal.options.fontFamily = family;
  terminal.options.fontSize = size;
  terminal.clearTextureAtlas();
  return true;
}
