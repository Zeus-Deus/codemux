import { useEffect } from "react";
import {
  EMPTY_APPEARANCE,
  useProjectAppearanceStore,
  type ProjectAppearance,
} from "@/stores/project-appearance-store";

export type { ProjectAppearance };

/**
 * Loads a project's user-customised avatar appearance (color + image +
 * cache-bust token).
 *
 * Backed by the shared `project-appearance-store`, so every surface rendering
 * the same project — inbox cards, settled rows, the collapsed rail, the
 * project filter dropdown — repaints together the moment the context menu
 * writes a new image or color. The store dedupes the underlying read, so it
 * costs one DB round-trip per project no matter how many avatars mount.
 */
export function useProjectAppearance(projectPath: string): ProjectAppearance {
  const load = useProjectAppearanceStore((s) => s.load);

  useEffect(() => {
    void load(projectPath);
  }, [load, projectPath]);

  return useProjectAppearanceStore(
    (s) => s.byPath[projectPath] ?? EMPTY_APPEARANCE,
  );
}
