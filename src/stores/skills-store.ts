import { create } from "zustand";

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

  loadSkills: (
    projectRoot: string | null,
    force?: boolean,
  ) => Promise<void>;
  setIncludePlugins: (value: boolean) => void;
  /** Drop the cache without re-fetching. Next `loadSkills` call refetches. */
  invalidate: () => void;
}

export const TTL_MS = 60_000;

export const useSkillsStore = create<SkillsState>((set, get) => ({
  skills: [],
  loaded: false,
  loading: false,
  error: null,
  loadedAt: 0,
  includePlugins: true,

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

  invalidate: () => {
    set({ loaded: false, loadedAt: 0 });
  },
}));
