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
  adapterErrors: Array<{ provider: Skill["provider"]; message: string }>;
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

  /** In-memory inventories are isolated by cwd and discovery options. */
  inventoryCache: Record<
    string,
    {
      skills: Skill[];
      adapterErrors: Array<{ provider: Skill["provider"]; message: string }>;
      loadedAt: number;
    }
  >;
  activeContextKey: string | null;
  inFlightContexts: Record<string, number>;
  nextRequestId: number;
  cacheGeneration: number;

  loadSkills: (projectRoot: string | null, force?: boolean) => Promise<void>;
  setIncludePlugins: (value: boolean) => void;
  /** Toggle a skill's disabled state. Idempotent — calling on an
   *  already-disabled id with the opposite value flips it back. */
  toggleSkillDisabled: (id: string) => void;
  /** Drop the cache without re-fetching. Next `loadSkills` call refetches. */
  invalidate: () => void;
}

export const TTL_MS = 60_000;

const STORAGE_KEY = "codemux:skills:v1";

/** Opportunistically migrate the pre-inventory path-hash preference to the
 * canonical-repository preference id. This fires when that same skill path is
 * rediscovered, preserving an existing disabled choice across the upgrade. */
export function migrateDisabledSkillIds(
  disabledIds: string[],
  skills: Skill[],
): string[] {
  if (disabledIds.length === 0) return disabledIds;
  const migrated = new Set(disabledIds);
  let changed = false;
  for (const skill of skills) {
    const preferenceId = skill.preferenceId;
    if (!preferenceId || preferenceId === skill.id || !migrated.has(skill.id)) {
      continue;
    }
    migrated.delete(skill.id);
    migrated.add(preferenceId);
    changed = true;
  }
  return changed ? [...migrated].sort() : disabledIds;
}

export const useSkillsStore = create<SkillsState>()(
  persist(
    (set, get) => ({
      skills: [],
      loaded: false,
      loading: false,
      error: null,
      adapterErrors: [],
      loadedAt: 0,
      includePlugins: true,
      disabledIds: [],
      inventoryCache: {},
      activeContextKey: null,
      inFlightContexts: {},
      nextRequestId: 1,
      cacheGeneration: 0,

      loadSkills: async (projectRoot, force = false) => {
        const state = get();
        const now = Date.now();
        const contextKey = JSON.stringify([
          projectRoot,
          state.includePlugins,
        ]);
        const cached = state.inventoryCache[contextKey];
        const fresh = cached && now - cached.loadedAt < TTL_MS;

        if (fresh && !force) {
          set({
            skills: cached.skills,
            adapterErrors: cached.adapterErrors,
            loaded: true,
            loading: false,
            error: null,
            loadedAt: cached.loadedAt,
            activeContextKey: contextKey,
          });
          return;
        }
        if (state.inFlightContexts[contextKey] !== undefined) {
          const sameVisibleContext = state.activeContextKey === contextKey;
          set({
            activeContextKey: contextKey,
            skills: cached?.skills ?? (sameVisibleContext ? state.skills : []),
            adapterErrors:
              cached?.adapterErrors ??
              (sameVisibleContext ? state.adapterErrors : []),
            loaded: Boolean(cached),
            loadedAt: cached?.loadedAt ?? 0,
            loading: true,
            error: null,
          });
          return;
        }

        const requestId = state.nextRequestId;
        const generation = state.cacheGeneration;
        const sameVisibleContext = state.activeContextKey === contextKey;
        set({
          activeContextKey: contextKey,
          skills: cached?.skills ?? (sameVisibleContext ? state.skills : []),
          adapterErrors:
            cached?.adapterErrors ??
            (sameVisibleContext ? state.adapterErrors : []),
          loaded: Boolean(cached),
          loadedAt: cached?.loadedAt ?? 0,
          loading: true,
          error: null,
          inFlightContexts: {
            ...state.inFlightContexts,
            [contextKey]: requestId,
          },
          nextRequestId: requestId + 1,
        });
        try {
          const response = await listSkills(
            projectRoot,
            get().includePlugins,
            force,
          );
          const inventory = Array.isArray(response)
            ? { skills: response as Skill[], errors: [] }
            : response;
          const loadedAt = Date.now();
          set((current) => {
            if (
              current.cacheGeneration !== generation ||
              current.inFlightContexts[contextKey] !== requestId
            ) {
              return {};
            }
            const inFlightContexts = { ...current.inFlightContexts };
            delete inFlightContexts[contextKey];
            const inventoryCache = {
              ...current.inventoryCache,
              [contextKey]: {
                skills: inventory.skills,
                adapterErrors: inventory.errors,
                loadedAt,
              },
            };
            if (current.activeContextKey !== contextKey) {
              return {
                inventoryCache,
                inFlightContexts,
                disabledIds: migrateDisabledSkillIds(
                  current.disabledIds,
                  inventory.skills,
                ),
              };
            }
            return {
              inventoryCache,
              inFlightContexts,
              skills: inventory.skills,
              adapterErrors: inventory.errors,
              loaded: true,
              loadedAt,
              loading: false,
              disabledIds: migrateDisabledSkillIds(
                current.disabledIds,
                inventory.skills,
              ),
            };
          });
        } catch (err) {
          set((current) => {
            if (
              current.cacheGeneration !== generation ||
              current.inFlightContexts[contextKey] !== requestId
            ) {
              return {};
            }
            const inFlightContexts = { ...current.inFlightContexts };
            delete inFlightContexts[contextKey];
            if (current.activeContextKey !== contextKey) {
              return { inFlightContexts };
            }
            return {
              inFlightContexts,
              loading: false,
              error: err instanceof Error ? err.message : String(err),
            };
          });
        }
      },

      setIncludePlugins: (value) => {
        if (get().includePlugins === value) return;
        set((state) => ({
          includePlugins: value,
          skills: [],
          adapterErrors: [],
          loaded: false,
          loading: false,
          loadedAt: 0,
          activeContextKey: null,
          inventoryCache: {},
          inFlightContexts: {},
          cacheGeneration: state.cacheGeneration + 1,
        }));
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
        set((state) => ({
          loaded: false,
          loading: false,
          loadedAt: 0,
          inventoryCache: {},
          inFlightContexts: {},
          cacheGeneration: state.cacheGeneration + 1,
        }));
      },
    }),
    {
      name: STORAGE_KEY,
      // Persist only the user's preferences. The skills list itself
      // is hydrated from disk on demand and would only get stale; the
      // loading/error/loadedAt state is per-session. Persisted shape
      // is the minimal subset the UI cares about across launches.
      storage: createJSONStorage(() => window.localStorage),
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
  return s.skills.filter(
    (skill) => !s.disabledIds.includes(skill.preferenceId ?? skill.id),
  );
};
