import { useEffect, useState } from "react";
import { getCurrentTheme, getShellAppearance } from "@/tauri/commands";
import { onThemeChanged } from "@/tauri/events";
import type { ThemeColors, ShellAppearance } from "@/tauri/types";
import { useTauriEvent } from "./use-tauri-event";

const fallbackTheme: ThemeColors = {
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

export function useThemeColors() {
  const [theme, setTheme] = useState<ThemeColors>(fallbackTheme);
  const [shellAppearance, setShellAppearance] = useState<ShellAppearance>({
    font_family: "monospace",
  });

  useEffect(() => {
    getCurrentTheme()
      .then(setTheme)
      .catch(() => setTheme(fallbackTheme));
    getShellAppearance()
      .then(setShellAppearance)
      .catch(() => {});
  }, []);

  useTauriEvent(onThemeChanged, setTheme, []);

  return { theme, shellAppearance };
}

export { fallbackTheme };
