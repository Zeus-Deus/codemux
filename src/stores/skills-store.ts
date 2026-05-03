import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";

import { listSkills, type Skill } from "@/tauri/commands";

/**
 * Cross-provider skill registry, lazily populated by calling the Rust
 * `list_skills` command on first popup open.
 *
 * The store caches results for {@link TTL_MS} so that re-opening the
 * slash popup multiple times in a row doesn't re-walk the disk. The
 * Settings UI (Stage 4) will own a manual refresh button via
 * `loadSkills(root, true)` and a project switcher via `invalidate()`.
 *
 * `includePlugins` is the Settings toggle for plugin-bundled Claude
 * skills (`~/.claude/plugins/...`). It defaults ON per the locked
 * Stage 7 decisions; flipping it invalidates the cache so the next
 * load reflects the new include-set.
 */
interface SkillsState {
  skills: Skill[];
  loaded: boolean;
  loading: boolean;
  error: string | null;
  /** epoch-ms of the last successful load. Compared against {@link TTL_MS}. */
  loadedAt: number;
  includePlugins: boolean;
  /**
   * Skill ids the user has explicitly disabled in Settings. Disabled
   * skills stay discovered (so the row keeps rendering, greyed) but
   * are filtered out of the slash popup so they can't be picked.
   * Stored sorted for deterministic localStorage payloads.
   */
  disabledIds: string[];

  loadSkills: (
    projectRoot: string | null,
    force?: boolean,
  ) => Promise<void>;
  setIncludePlugins: (value: boolean) => void;
  /** Toggle a skill's disabled state. Idempotent — calling on an
   *  already-disabled id with the opposite value flips it back. */
  toggleSkillDisabled: (id: string) => void;
  /** Drop the cache without re-fetching. Next `loadSkills` call refetches. */
  invalidate: () => void;
}

export const TTL_MS = 60_000;

const STORAGE_KEY = "codemux:skills:v1";

export const useSkillsStore = create<SkillsState>()(
  persist(
    (set, get) => ({
      skills: [],
      loaded: false,
      loading: false,
      error: null,
      loadedAt: 0,
      includePlugins: true,
      disabledIds: [],

      loadSkills: async (projectRoot, force = false) => {
        const state = get();
        const now = Date.now();
        const fresh = state.loaded && now - state.loadedAt < TTL_MS;

        if (state.loading) return;
        if (fresh && !force) return;

        set({ loading: true, error: null });
        try {
          const skills = await listSkills(projectRoot, get().includePlugins);
          set({
            skills,
            loaded: true,
            loadedAt: Date.now(),
            loading: false,
          });
        } catch (err) {
          set({
            loading: false,
            error: err instanceof Error ? err.message : String(err),
          });
        }
      },

      setIncludePlugins: (value) => {
        if (get().includePlugins === value) return;
        set({ includePlugins: value, loaded: false, loadedAt: 0 });
      },

      toggleSkillDisabled: (id) => {
        const current = get().disabledIds;
        const isDisabled = current.includes(id);
        const next = isDisabled
          ? current.filter((x) => x !== id)
          : [...current, id].sort();
        set({ disabledIds: next });
      },

      invalidate: () => {
        set({ loaded: false, loadedAt: 0 });
      },
    }),
    {
      name: STORAGE_KEY,
      // Persist only the user's preferences. The skills list itself
      // is hydrated from disk on demand and would only get stale; the
      // loading/error/loadedAt state is per-session. Persisted shape
      // is the minimal subset the UI cares about across launches.
      storage: createJSONStorage(() => localStorage),
      partialize: (s) => ({
        includePlugins: s.includePlugins,
        disabledIds: s.disabledIds,
      }),
    },
  ),
);

/**
 * Selector returning skills the user has NOT disabled. Used by every
 * surface that should respect the user's enable/disable choice — the
 * slash popup, inline highlight overlay, and send-time body resolver.
 * The Settings UI uses `state.skills` directly so disabled skills can
 * still render (greyed) with their toggle to switch back on.
 */
export const selectActiveSkills = (s: SkillsState): Skill[] => {
  if (s.disabledIds.length === 0) return s.skills;
  return s.skills.filter((skill) => !s.disabledIds.includes(skill.id));
};
