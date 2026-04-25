/**
 * App-level theme MutationObserver for the terminal cache.
 *
 * Theme changes (light/dark, accent updates) are signalled by class/style
 * mutations on document.documentElement. This hook installs ONE observer
 * that walks every cached xterm and updates its theme — including panes
 * that are currently unmounted/parked. Without it, switching theme while
 * a workspace pane is unmounted leaves it stuck on the old palette.
 *
 * The TerminalPane component still has a per-mount observer for the live
 * theme of the visible pane (cheap belt-and-suspenders), but this hook is
 * the source of truth for cached-but-unmounted panes.
 */
import { useEffect } from "react";
import type { ITheme } from "@xterm/xterm";
import { applyThemeToAllTerminals } from "@/components/terminal/terminal-cache";

const ANSI_COLORS = {
  black: "#151110",
  red: "#dc6b6b",
  green: "#7ec699",
  yellow: "#e5c07b",
  blue: "#61afef",
  magenta: "#c678dd",
  cyan: "#56b6c2",
  white: "#eae8e6",
  brightBlack: "#5c5856",
  brightRed: "#e88888",
  brightGreen: "#98d1a8",
  brightYellow: "#ecd08f",
  brightBlue: "#7ec0f5",
  brightMagenta: "#d494e6",
  brightCyan: "#73c7d3",
  brightWhite: "#ffffff",
};

function resolveOklch(value: string): string {
  const el = document.createElement("div");
  el.style.color = value;
  document.body.appendChild(el);
  const rgb = getComputedStyle(el).color;
  document.body.removeChild(el);
  return rgb;
}

function getCSSVar(name: string): string {
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  if (!raw) return "";
  return resolveOklch(raw);
}

function buildThemeFromCSS(): ITheme {
  return {
    background: getCSSVar("--background"),
    foreground: getCSSVar("--foreground"),
    cursor: getCSSVar("--sidebar-primary"),
    cursorAccent: getCSSVar("--background"),
    selectionBackground: getCSSVar("--accent"),
    selectionForeground: getCSSVar("--accent-foreground"),
    ...ANSI_COLORS,
  };
}

export function useTerminalThemeSync() {
  useEffect(() => {
    const observer = new MutationObserver(() => {
      applyThemeToAllTerminals(buildThemeFromCSS());
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    return () => observer.disconnect();
  }, []);
}
