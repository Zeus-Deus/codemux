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

/** Per-project set of fields a setter has explicitly written. `load` consults
 *  this to merge field-by-field: written fields keep their in-store value,
 *  everything else takes the persisted value. A dirty-field set (not a
 *  null-coalescing merge) because `null` is a legitimate written value —
 *  clearing the color must survive a load resolving afterwards. Kept outside
 *  the store for the same reason as `loadStarted`. */
const writtenFields = new Map<string, Set<keyof ProjectAppearance>>();

function markWritten(projectPath: string, ...fields: (keyof ProjectAppearance)[]) {
  const set = writtenFields.get(projectPath) ?? new Set();
  for (const field of fields) set.add(field);
  writtenFields.set(projectPath, set);
}

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
        // Merge field-by-field: a write that landed while this read was in
        // flight wins for the field it wrote — the user's just-picked color
        // must never be clobbered by the stale stored value — but fields the
        // user has not touched still take their persisted values. Discarding
        // the whole read on any prior write would drop the persisted image
        // when a color pick raced the load (and vice versa).
        const existing = s.byPath[projectPath] ?? EMPTY_APPEARANCE;
        const written = writtenFields.get(projectPath);
        const merged: ProjectAppearance = {
          // Cleared values are persisted as "" rather than deleted, so
          // normalise falsy to null.
          customColor: written?.has("customColor")
            ? existing.customColor
            : color || null,
          imageUrl: written?.has("imageUrl") ? existing.imageUrl : image || null,
          imageVersion: written?.has("imageVersion")
            ? existing.imageVersion
            : version || null,
        };
        if (
          merged.customColor === existing.customColor &&
          merged.imageUrl === existing.imageUrl &&
          merged.imageVersion === existing.imageVersion &&
          s.byPath[projectPath]
        ) {
          return s; // Nothing changed — keep the snapshot reference stable.
        }
        return { byPath: { ...s.byPath, [projectPath]: merged } };
      });
    },

    setColor: (projectPath, color) => {
      markWritten(projectPath, "customColor");
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
      markWritten(projectPath, "imageUrl", "imageVersion");
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

/** Test-only reset — clears cached state, the load-dedupe set, and the
 *  dirty-field bookkeeping. */
export function __resetProjectAppearanceStoreForTests() {
  loadStarted.clear();
  writtenFields.clear();
  useProjectAppearanceStore.setState({ byPath: {} });
}
