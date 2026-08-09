import type { ITheme } from "@xterm/xterm";
import type { ThemeColors } from "@/tauri/types";

/** Maps the shared app/system syntax palette into xterm's named slots. */
export function themeColorsToXtermTheme(theme: ThemeColors): ITheme {
  return {
    background: theme.background,
    foreground: theme.foreground,
    cursor: theme.cursor,
    cursorAccent: theme.background,
    selectionBackground: theme.selection_background,
    selectionForeground: theme.selection_foreground,
    black: theme.color0,
    red: theme.color1,
    green: theme.color2,
    yellow: theme.color3,
    blue: theme.color4,
    magenta: theme.color5,
    cyan: theme.color6,
    white: theme.color7,
    brightBlack: theme.color8,
    brightRed: theme.color9,
    brightGreen: theme.color10,
    brightYellow: theme.color11,
    brightBlue: theme.color12,
    brightMagenta: theme.color13,
    brightCyan: theme.color14,
    brightWhite: theme.color15,
  };
}
