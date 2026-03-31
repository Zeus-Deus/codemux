import { useMemo } from "react";
import { KEYBIND_REGISTRY, type KeybindEntry } from "@/lib/keybind-registry";
import {
  useSyncedSettingsStore,
  selectKeyboardShortcuts,
} from "@/stores/synced-settings-store";

export interface ResolvedEntry extends KeybindEntry {
  activeKeys: string;
  isCustom: boolean;
}

export interface ResolvedKeybinds {
  /** id → resolved entry */
  keybindMap: Map<string, ResolvedEntry>;
  /** combo → action IDs (for conflict detection) */
  reverseMap: Map<string, string[]>;
  /** Combos bound to 2+ actions */
  conflicts: Array<{ combo: string; ids: string[] }>;
  getKeysForAction: (id: string) => string;
  getActionForKeys: (keys: string) => string | null;
}

/**
 * Non-hook version: merges defaults + overrides.
 * Usable outside React (app-shortcuts, tests).
 */
export function resolveKeybinds(
  overrides: Record<string, string>,
): ResolvedKeybinds {
  const keybindMap = new Map<string, ResolvedEntry>();
  const reverseMap = new Map<string, string[]>();

  for (const entry of KEYBIND_REGISTRY) {
    const hasOverride = entry.id in overrides;
    const activeKeys = hasOverride ? overrides[entry.id] : entry.defaultKeys;
    const isCustom = hasOverride && overrides[entry.id] !== entry.defaultKeys;

    keybindMap.set(entry.id, { ...entry, activeKeys, isCustom });

    if (activeKeys) {
      const ids = reverseMap.get(activeKeys) ?? [];
      ids.push(entry.id);
      reverseMap.set(activeKeys, ids);
    }
  }

  const conflicts: Array<{ combo: string; ids: string[] }> = [];
  for (const [combo, ids] of reverseMap) {
    if (ids.length > 1) conflicts.push({ combo, ids });
  }

  return {
    keybindMap,
    reverseMap,
    conflicts,
    getKeysForAction: (id) => keybindMap.get(id)?.activeKeys ?? "",
    getActionForKeys: (keys) => {
      const ids = reverseMap.get(keys);
      return ids?.[0] ?? null;
    },
  };
}

/** React hook: resolved keybinds that react to settings changes */
export function useResolvedKeybinds(): ResolvedKeybinds {
  const overrides = useSyncedSettingsStore(selectKeyboardShortcuts);
  return useMemo(() => resolveKeybinds(overrides), [overrides]);
}
