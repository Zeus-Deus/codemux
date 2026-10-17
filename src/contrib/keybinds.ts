/**
 * Plugin keybinds. Contributions in the `keybinds` area become extra
 * `KeybindEntry` rows (id `plugin:<pluginId>:<localId>`), so Settings →
 * Shortcuts lists and rebinds them like core actions, and the global keydown
 * handler falls through to `runPluginKeybind` for ids it doesn't own.
 */
import { registry } from "./registry";
import { reportPluginError } from "./activation";
import { pluginIdOf, pluginLabel } from "./attribution";
import { setExtraKeybindProvider, type KeybindEntry } from "@/lib/keybind-registry";
import { normalizeKeyName } from "@/lib/keybind-utils";
import { KEYBINDS_AREA } from "@/sdk/areas";
import type { KeybindData } from "@/sdk/host";

const PREFIX = "plugin:";

export function pluginKeybindId(contributionId: string): string {
  return `${PREFIX}${contributionId}`;
}

/**
 * Canonical combo form (modifiers in Alt, Ctrl, Shift order, key normalised),
 * which is what keydown produces. Plugins may write "Ctrl+Alt+E" or "ctrl+alt+e".
 */
export function canonicalCombo(keys: string): string {
  if (!keys) return "";
  const parts = keys.split("+");
  // "Ctrl++" ends in an empty part pair: the key itself is "+".
  const key = keys.endsWith("++") ? "+" : (parts.pop() ?? "");
  const mods = new Set(parts.filter(Boolean).map((m) => m.toLowerCase()));
  const out: string[] = [];
  if (mods.has("alt") || mods.has("option")) out.push("Alt");
  if (mods.has("ctrl") || mods.has("control") || mods.has("mod") || mods.has("cmd")) out.push("Ctrl");
  if (mods.has("shift")) out.push("Shift");
  out.push(normalizeKeyName(key));
  return out.join("+");
}

let cachedFor: readonly unknown[] | null = null;
let cachedEntries: readonly KeybindEntry[] = [];

/** Current plugin keybind entries. Same array until the area changes. */
export function pluginKeybindEntries(): readonly KeybindEntry[] {
  const items = registry.getArea(KEYBINDS_AREA);
  if (items === cachedFor) return cachedEntries;
  cachedFor = items;
  cachedEntries = items.flatMap((c) => {
    const data = c.data as KeybindData | undefined;
    if (!data || typeof data.label !== "string" || typeof data.run !== "function") return [];
    return [
      {
        id: pluginKeybindId(c.id),
        label: `${data.label} · ${pluginLabel(pluginIdOf(c.id))}`,
        category: data.category ?? "general",
        defaultKeys: canonicalCombo(data.defaultKeys ?? ""),
        description: data.description,
      } satisfies KeybindEntry,
    ];
  });
  return cachedEntries;
}

export function subscribePluginKeybinds(fn: () => void): () => void {
  return registry.subscribeArea(KEYBINDS_AREA, fn);
}

/** Run a plugin keybind. Returns false for ids that aren't plugin keybinds. */
export function runPluginKeybind(actionId: string, event?: KeyboardEvent): boolean {
  if (!actionId.startsWith(PREFIX)) return false;
  const id = actionId.slice(PREFIX.length);
  const c = registry.getArea(KEYBINDS_AREA).find((x) => x.id === id);
  const data = c?.data as KeybindData | undefined;
  if (!data) return false;
  try {
    return data.run(event) !== false;
  } catch (error) {
    reportPluginError(pluginLabel(pluginIdOf(id)), "shortcut failed", error);
    return true;
  }
}

// Plugin entries join the keybind registry. This module is imported by the
// keyboard hook at startup; with no plugin keybinds the provider returns one
// stable empty array, so the registry stays `KEYBIND_REGISTRY` itself.
setExtraKeybindProvider(pluginKeybindEntries);
