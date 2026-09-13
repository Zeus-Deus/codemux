import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { OmarchyTheme } from "@/lib/omarchy-theme";

export const getOmarchyTheme = () => invoke<OmarchyTheme | null>("get_omarchy_theme");
export const onOmarchyThemeChanged = (handler: (theme: OmarchyTheme) => void) =>
  listen<OmarchyTheme>("omarchy-theme-changed", (event) => handler(event.payload));
