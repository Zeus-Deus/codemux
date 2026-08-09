import { useSyncExternalStore } from "react";
import { getCurrentTheme, getShellAppearance } from "@/tauri/commands";
import { onThemeChanged } from "@/tauri/events";
import type { ThemeColors, ShellAppearance } from "@/tauri/types";
import {
  getActiveTheme,
  subscribeActiveTheme,
  themeToSyntaxColors,
  type ThemeDefinition,
} from "@/lib/themes";
import {
  getTerminalThemeMode,
  subscribeTerminalThemeMode,
} from "@/lib/terminal-theme-mode";

export const fallbackTheme: ThemeColors = {
  accent: "#7aa2f7",
  cursor: "#c0caf5",
  foreground: "#c0caf5",
  background: "#1a1b26",
  selection_foreground: "#c0caf5",
  selection_background: "#283457",
  color0: "#15161e",
  color1: "#f7768e",
  color2: "#9ece6a",
  color3: "#e0af68",
  color4: "#7aa2f7",
  color5: "#bb9af7",
  color6: "#7dcfff",
  color7: "#a9b1d6",
  color8: "#414868",
  color9: "#f7768e",
  color10: "#9ece6a",
  color11: "#e0af68",
  color12: "#7aa2f7",
  color13: "#bb9af7",
  color14: "#7dcfff",
  color15: "#c0caf5",
};

let systemTheme = fallbackTheme;
let shellAppearance: ShellAppearance = { font_family: "monospace" };
let started = false;
const systemListeners = new Set<() => void>();
const shellListeners = new Set<() => void>();

function notify(listeners: Set<() => void>) {
  for (const listener of listeners) listener();
}

function startSystemThemeStore() {
  if (started) return;
  started = true;
  if (typeof getCurrentTheme === "function") getCurrentTheme().then((theme) => {
    systemTheme = theme;
    notify(systemListeners);
  }).catch(() => {});
  if (typeof getShellAppearance === "function") getShellAppearance().then((appearance) => {
    shellAppearance = appearance;
    notify(shellListeners);
  }).catch(() => {});
  if (typeof onThemeChanged === "function") onThemeChanged((theme) => {
    systemTheme = theme;
    notify(systemListeners);
  }).catch(() => {});
}

export function subscribeSystemTheme(listener: () => void): () => void {
  systemListeners.add(listener);
  startSystemThemeStore();
  return () => systemListeners.delete(listener);
}

export function getSystemThemeSnapshot(): ThemeColors {
  return systemTheme;
}

function subscribeShellAppearance(listener: () => void): () => void {
  shellListeners.add(listener);
  startSystemThemeStore();
  return () => shellListeners.delete(listener);
}

let cachedActiveTheme: ThemeDefinition | null = null;
let cachedAppSyntax = fallbackTheme;

function appSyntaxSnapshot(): ThemeColors {
  const active = getActiveTheme();
  if (active !== cachedActiveTheme) {
    cachedActiveTheme = active;
    cachedAppSyntax = themeToSyntaxColors(active);
  }
  return cachedAppSyntax;
}

export function getSyntaxThemeSnapshot(): ThemeColors {
  return getTerminalThemeMode() === "system"
    ? systemTheme
    : appSyntaxSnapshot();
}

export function subscribeSyntaxTheme(listener: () => void): () => void {
  startSystemThemeStore();
  const offSystem = subscribeSystemTheme(listener);
  const offActive = subscribeActiveTheme(listener);
  const offSettings = subscribeTerminalThemeMode(listener);
  return () => {
    offSystem();
    offActive();
    offSettings();
  };
}

export function useThemeColors() {
  const theme = useSyncExternalStore(subscribeSystemTheme, getSystemThemeSnapshot, getSystemThemeSnapshot);
  const appearance = useSyncExternalStore(subscribeShellAppearance, () => shellAppearance, () => shellAppearance);
  return { theme, shellAppearance: appearance };
}

/** ANSI/syntax palette selected by Settings → Terminal: the active app theme
 * or the desktop/Omarchy theme. Shared by xterm, Shiki, and CodeMirror. */
export function useSyntaxThemeColors(): ThemeColors {
  return useSyncExternalStore(subscribeSyntaxTheme, getSyntaxThemeSnapshot, getSyntaxThemeSnapshot);
}
