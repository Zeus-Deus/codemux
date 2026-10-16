/**
 * `themes`: plugin-contributed themes. Each `data` payload is validated and
 * completed with the same parser as imported custom themes, and its id is
 * namespaced (`plugin:<pluginId>:<id>`) so it can never shadow a built-in or
 * a user theme. Plugin themes are listed in the picker and never selected
 * automatically; if the selected one's plugin turns off, the app falls back
 * the same way it does for a deleted custom theme.
 */
import { useSyncExternalStore } from "react";
import { parseCustomTheme, type ThemeDefinition } from "@/lib/themes";
import { THEMES_AREA } from "@/sdk/areas";
import { registry } from "./registry";

const EMPTY: readonly ThemeDefinition[] = Object.freeze([]);
let cachedFor: readonly unknown[] | null = null;
let cached: readonly ThemeDefinition[] = EMPTY;

export function pluginThemes(): readonly ThemeDefinition[] {
  const items = registry.getArea(THEMES_AREA);
  if (items === cachedFor) return cached;
  cachedFor = items;
  const out: ThemeDefinition[] = [];
  for (const c of items) {
    const parsed = parseCustomTheme(c.data);
    if (!parsed) {
      console.warn(`[plugins] ${c.id}: theme data is not a valid theme; skipped`);
      continue;
    }
    const id = `plugin:${c.id}`;
    if (!out.some((t) => t.id === id)) out.push({ ...parsed, id });
  }
  cached = out.length ? out : EMPTY;
  return cached;
}

const subscribe = (fn: () => void) => registry.subscribeArea(THEMES_AREA, fn);

/** Plugin themes, reactive. The same frozen [] while no plugin adds one. */
export function usePluginThemes(): readonly ThemeDefinition[] {
  return useSyncExternalStore(subscribe, pluginThemes, pluginThemes);
}

/** `custom` plus plugin themes; returns `custom` itself when there are none. */
export function withPluginThemes(
  custom: ThemeDefinition[],
  plugin: readonly ThemeDefinition[],
): ThemeDefinition[] {
  return plugin.length === 0 ? custom : [...custom, ...plugin];
}
