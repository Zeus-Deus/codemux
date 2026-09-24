import type { ChatFileLinkMeta } from "./file-links";

function basename(path: string): string {
  return path.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? path;
}

/** The parent directory of an absolute path, keeping its separator style. */
function dirname(path: string): string | null {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : null;
}

function isBareName(meta: ChatFileLinkMeta): boolean {
  return !/[\\/]/.test(meta.displayPath);
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
 *    tool calls and the message itself mentioned (`referencePaths`).
 * 4. For a bare filename only: the same name inside the directory of each
 *    of those paths, newest first. Agents write "`/tmp/shots/a.png`, plus
 *    `b.gif` in the same folder", and a file built by `cd dir && … b.gif`
 *    never has its absolute path spelled out anywhere.
 * 5. `null`: the caller must not open a tab for a file nothing can load.
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

  if (!isBareName(meta)) return null;
  const tried = new Set<string>();
  for (let index = referencePaths.length - 1; index >= 0; index -= 1) {
    const directory = dirname(referencePaths[index]);
    if (!directory || tried.has(directory)) continue;
    tried.add(directory);
    const separator = directory.includes("\\") ? "\\" : "/";
    const sibling = `${directory}${separator}${meta.basename}`;
    if (sibling === meta.filePath) continue;
    if (await probe(sibling)) return sibling;
  }
  return null;
}
