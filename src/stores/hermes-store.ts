import { create } from "zustand";
import { persist } from "zustand/middleware";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "./settings-store";

export interface HermesProfile {
  schema_version: number;
  host: string;
  installation: string;
  root: string;
  id: string;
  home: string;
  identity: string;
}
export interface HermesCatalog {
  state: "ready" | "unsupported";
  message: string | null;
  session: { modes?: { currentModeId: string; availableModes: Array<{ id: string; name: string; description?: string }> }; models?: { currentModelId: string; availableModels: Array<{ modelId: string; name: string; description?: string; _meta?: { provider?: string } }> } };
}
export const hermesProfileKey = (p: HermesProfile) => JSON.stringify([p.host, p.installation, p.root, p.home, p.identity]);
export const HERMES_DEFAULT_PROFILE_SETTING = "hermes.default_profile";
/** Last explicit pick in this project, then the Settings default, then the only installed profile. */
export function pickHermesProfile(profiles: HermesProfile[], project: string): HermesProfile | null {
  const preferred = useHermes.getState().preferred[project];
  const remembered = preferred && profiles.find(p => hermesProfileKey(p) === hermesProfileKey(preferred));
  if (remembered) return remembered;
  const id = useSettingsStore.getState().settings[HERMES_DEFAULT_PROFILE_SETTING];
  const configured = id && profiles.find(p => p.id === id);
  if (configured) return configured;
  return profiles.length === 1 ? profiles[0] : null;
}
export const hermesModelUnavailable = (id: string) => id.startsWith("custom:") && id.slice(7).includes(":");
interface CatalogSlot { loading: boolean; error: string | null; value: HermesCatalog | null }
interface HermesStore {
  selections: Record<string, HermesProfile>;
  preferred: Record<string, HermesProfile>;
  fixed: Record<string, boolean>;
  modes: Record<string, string>;
  catalogs: Record<string, CatalogSlot>;
  /** Installed profiles as last listed by Settings; not persisted. */
  profiles: HermesProfile[] | null;
  loadProfiles: () => Promise<HermesProfile[]>;
  /** `remember: false` keeps automatic picks from overriding later default changes. */
  select: (thread: string, project: string, profile: HermesProfile, remember?: boolean) => void;
  restore: (thread: string) => Promise<void>;
  refresh: (profile: HermesProfile) => Promise<void>;
}
const generations = new Map<string, number>();
export const useHermes = create<HermesStore>()(persist((set, get) => ({
  selections: {}, preferred: {}, fixed: {}, modes: {}, catalogs: {}, profiles: null,
  async loadProfiles() {
    const profiles = await invoke<HermesProfile[]>("hermes_profiles");
    set({ profiles });
    return profiles;
  },
  select(thread, project, profile, remember = true) {
    if (get().fixed[thread]) return;
    set(s => {
      const modes = {...s.modes};
      if (!s.selections[thread] || hermesProfileKey(s.selections[thread]) !== hermesProfileKey(profile)) delete modes[thread];
      return { selections: { ...s.selections, [thread]: profile }, preferred: remember ? { ...s.preferred, [project]: profile } : s.preferred, modes };
    });
  },
  async restore(thread) {
    const binding = await invoke<{ profile: HermesProfile; permission_mode?: string } | null>("hermes_binding", { threadId: thread });
    if (binding) set(s => ({ selections: { ...s.selections, [thread]: binding.profile }, fixed: { ...s.fixed, [thread]: true }, modes: binding.permission_mode ? { ...s.modes, [thread]: binding.permission_mode } : s.modes }));
  },
  async refresh(profile) {
    const key = hermesProfileKey(profile);
    const generation = (generations.get(key) ?? 0) + 1;
    generations.set(key, generation);
    set(s => ({ catalogs: { ...s.catalogs, [key]: { loading: true, error: null, value: s.catalogs[key]?.value ?? null } } }));
    try {
      const value = await invoke<HermesCatalog>("hermes_catalog", { profile });
      if (generations.get(key) !== generation) return;
      set(s => ({ catalogs: { ...s.catalogs, [key]: { loading: false, error: null, value } } }));
    } catch (error) {
      if (generations.get(key) !== generation) return;
      set(s => ({ catalogs: { ...s.catalogs, [key]: { loading: false, error: String(error), value: null } } }));
    }
  },
}), { name: "codemux:hermes:v1", partialize: s => ({ selections: s.selections, preferred: s.preferred, modes: s.modes }) }));
