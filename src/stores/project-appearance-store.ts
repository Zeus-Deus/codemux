import { create } from "zustand";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";

/** A project's user-customised avatar appearance. */
export interface ProjectAppearance {
  customColor: string | null;
  imageUrl: string | null;
  /** Cache-bust token for the derived favicon (bumped on every image save). */
  imageVersion: string | null;
}

/** Shared identity for an unset project. A frozen module-level singleton so
 *  selectors returning it keep a stable reference — a fresh object per render
 *  would make every `useProjectAppearance` consumer re-render forever. */
export const EMPTY_APPEARANCE: ProjectAppearance = Object.freeze({
  customColor: null,
  imageUrl: null,
  imageVersion: null,
});

// Persisted UI-state keys, all suffixed with the absolute project path.
// Device-local (the `ui_state` table is `user_id = 'local'`), not synced.
const colorKey = (path: string) => `project.color:${path}`;
const imageKey = (path: string) => `project.image:${path}`;
const imageVersionKey = (path: string) => `project.image.v:${path}`;

interface ProjectAppearanceStore {
  /** Project path → appearance. Absent means "not loaded yet". */
  byPath: Record<string, ProjectAppearance>;
  /** Read a project's persisted appearance. Idempotent and deduped — safe to
   *  call from the mount effect of every avatar on screen. */
  load: (projectPath: string) => Promise<void>;
  /** Set (or clear, with null) the accent color. Writes through to UI state. */
  setColor: (projectPath: string, color: string | null) => void;
  /** Set (or clear, with null) the image. `value` is the raw user input — a
   *  direct image URL, a `data:` URL, or a bare domain whose favicon is
   *  derived at render time by `resolveImageUrl`. */
  setImage: (projectPath: string, value: string | null) => void;
}

/** Paths whose load has been started. Kept outside the store because it is
 *  bookkeeping, not rendered state — mutating it must never trigger a render. */
const loadStarted = new Set<string>();

/**
 * Per-project avatar appearance (image + accent color), shared by every
 * surface that renders a `ProjectAvatar`.
 *
 * This is a store rather than per-component state because the appearance has
 * one writer (the project section of the workspace context menu) and many
 * readers (inbox cards, settled rows, the collapsed rail, the project filter
 * dropdown). Setting an image on one card must repaint every other row of the
 * same project immediately — the previous read-only hook cached per component,
 * so a write only showed up on the next remount.
 */
export const useProjectAppearanceStore = create<ProjectAppearanceStore>(
  (set) => ({
    byPath: {},

    load: async (projectPath) => {
      if (loadStarted.has(projectPath)) return;
      loadStarted.add(projectPath);
      const [color, image, version] = await Promise.all([
        dbGetUiState(colorKey(projectPath)).catch(() => null),
        dbGetUiState(imageKey(projectPath)).catch(() => null),
        dbGetUiState(imageVersionKey(projectPath)).catch(() => null),
      ]);
      set((s) => {
        // A write that landed while this read was in flight wins — the user's
        // just-picked color must never be clobbered by the stale stored value.
        if (s.byPath[projectPath]) return s;
        return {
          byPath: {
            ...s.byPath,
            [projectPath]: {
              // Cleared values are persisted as "" rather than deleted, so
              // normalise falsy to null.
              customColor: color || null,
              imageUrl: image || null,
              imageVersion: version || null,
            },
          },
        };
      });
    },

    setColor: (projectPath, color) => {
      set((s) => ({
        byPath: {
          ...s.byPath,
          [projectPath]: {
            ...(s.byPath[projectPath] ?? EMPTY_APPEARANCE),
            customColor: color,
          },
        },
      }));
      dbSetUiState(colorKey(projectPath), color ?? "").catch(console.error);
    },

    setImage: (projectPath, value) => {
      // A new token on every save forces the favicon to refresh, so re-adding
      // a site whose icon changed picks up the new one instead of the cached
      // bytes. Direct/data URLs ignore it (see `resolveImageUrl`).
      const version = value ? String(Date.now()) : null;
      set((s) => ({
        byPath: {
          ...s.byPath,
          [projectPath]: {
            ...(s.byPath[projectPath] ?? EMPTY_APPEARANCE),
            imageUrl: value,
            imageVersion: version,
          },
        },
      }));
      dbSetUiState(imageKey(projectPath), value ?? "").catch(console.error);
      dbSetUiState(imageVersionKey(projectPath), version ?? "").catch(
        console.error,
      );
    },
  }),
);

/** Test-only reset — clears both cached state and the load-dedupe set. */
export function __resetProjectAppearanceStoreForTests() {
  loadStarted.clear();
  useProjectAppearanceStore.setState({ byPath: {} });
}
