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
 * 2. Newest-first basename match against absolute paths the same turn's
 *    tool calls mentioned (`assistantReferencePaths`).
 * 3. `null`: the caller must not open a tab for a file nothing can load.
 *
 * `exists` is injected (the `file_exists` Tauri command in production) so
 * resolution order stays testable without IPC.
 */
export async function resolveExistingFileTarget(
  meta: ChatFileLinkMeta,
  referencePaths: readonly string[],
  exists: (path: string) => Promise<boolean>,
): Promise<string | null> {
  if (await exists(meta.filePath)) return meta.filePath;

  for (let index = referencePaths.length - 1; index >= 0; index -= 1) {
    const candidate = referencePaths[index];
    if (candidate === meta.filePath) continue;
    if (basename(candidate) !== meta.basename) continue;
    if (await exists(candidate)) return candidate;
  }
  return null;
}
