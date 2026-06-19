import { useEffect, useState } from "react";
import { dbGetUiState } from "@/tauri/commands";

export interface ProjectAppearance {
  customColor: string | null;
  imageUrl: string | null;
  /** Cache-bust token for the derived favicon (bumped on every save). */
  imageVersion: string | null;
}

/**
 * Loads a project's user-customised avatar appearance (color + image +
 * cache-bust token) from persisted UI state.
 *
 * The expanded project group header owns its own copy of this state because
 * it also writes it (the context-menu color/image actions). The collapsed
 * rail avatar is read-only — its context menu lives on the expanded header,
 * which you can only reach while expanded — so it loads independently here
 * and picks up any change the next time the rail mounts (i.e. on collapse).
 */
export function useProjectAppearance(projectPath: string): ProjectAppearance {
  const [customColor, setCustomColor] = useState<string | null>(null);
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const [imageVersion, setImageVersion] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    dbGetUiState(`project.color:${projectPath}`)
      .then((v) => { if (!cancelled && v) setCustomColor(v); })
      .catch(() => {});
    dbGetUiState(`project.image:${projectPath}`)
      .then((v) => { if (!cancelled && v) setImageUrl(v); })
      .catch(() => {});
    dbGetUiState(`project.image.v:${projectPath}`)
      .then((v) => { if (!cancelled && v) setImageVersion(v); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [projectPath]);

  return { customColor, imageUrl, imageVersion };
}
