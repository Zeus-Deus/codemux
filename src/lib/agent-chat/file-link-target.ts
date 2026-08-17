import type { ChatFileLinkMeta } from "./file-links";

function basename(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? path;
}

/**
 * Click-time resolution for a chat file chip. The parsed target is a guess —
 * agents emit bare filenames for files that live outside the directory the
 * link resolves against (a screenshot written to a temp directory is the
 * canonical case). Layered, each step stat-verified:
 *
 * 1. The resolved path itself, when it exists on disk.
 * 2. The same reference resolved against the workspace root (`workspacePath`).
 *    The link's directory is the last tool working directory, which may be a
 *    subdirectory of the workspace; a repo-root file named in the prose has
 *    to keep opening.
 * 3. Newest-first basename match against absolute paths the same turn's
 *    tool calls mentioned (`assistantReferencePaths`).
 * 4. `null`: the caller must not open a tab for a file nothing can load.
 *
 * `exists` is injected (the `file_exists` Tauri command in production) so
 * resolution order stays testable without IPC.
 *
 * A rejection from the *first* probe means the check itself is unavailable
 * (e.g. an older remote backend); that returns the parsed path so the chip
 * still opens. Later rejections only disqualify their own candidate — once
 * the first probe has reported the parsed path missing, opening it anyway
 * would show a dead viewer.
 */
export async function resolveExistingFileTarget(
  meta: ChatFileLinkMeta,
  referencePaths: readonly string[],
  exists: (path: string) => Promise<boolean>,
): Promise<string | null> {
  try {
    if (await exists(meta.filePath)) return meta.filePath;
  } catch {
    return meta.filePath;
  }

  const probe = async (path: string): Promise<boolean> => {
    try {
      return await exists(path);
    } catch {
      return false;
    }
  };

  if (
    meta.workspacePath &&
    meta.workspacePath !== meta.filePath &&
    (await probe(meta.workspacePath))
  ) {
    return meta.workspacePath;
  }

  for (let index = referencePaths.length - 1; index >= 0; index -= 1) {
    const candidate = referencePaths[index];
    if (candidate === meta.filePath) continue;
    if (basename(candidate) !== meta.basename) continue;
    if (await probe(candidate)) return candidate;
  }
  return null;
}
