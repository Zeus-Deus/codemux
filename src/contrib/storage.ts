import { dbDeleteSetting } from "@/tauri/commands";
import { useSettingsStore } from "@/stores/settings-store";
import type { PluginStorage } from "./plugin";

/**
 * Per-plugin JSON storage in the machine-local settings DB (`settings`
 * table, never part of account sync) under `plugin.<id>.<key>`.
 */
export function pluginStorage(pluginId: string): PluginStorage {
  const scoped = (key: string) => `plugin.${pluginId}.${key}`;
  return {
    get<T>(key: string, fallback: T): T {
      const raw = useSettingsStore.getState().settings[scoped(key)];
      if (raw === undefined) return fallback;
      try {
        return JSON.parse(raw) as T;
      } catch {
        return fallback;
      }
    },
    set(key, value) {
      useSettingsStore.getState().set(scoped(key), JSON.stringify(value));
    },
    remove(key) {
      const full = scoped(key);
      useSettingsStore.setState((s) => {
        const { [full]: _removed, ...rest } = s.settings;
        return { settings: rest };
      });
      void dbDeleteSetting(full).catch((error: unknown) =>
        console.error(`[plugins] ${pluginId}: storage.remove failed`, error),
      );
    },
  };
}
