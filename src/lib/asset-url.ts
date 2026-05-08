import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * Helpers for turning user-authored image references into URLs the
 * Tauri webview can actually load.
 *
 * The webview cannot load `file://` URLs directly. Tauri's asset
 * protocol (`asset:` / `tauri:`) is the bridge — `convertFileSrc()`
 * builds the right scheme + host for the current platform. The
 * protocol must be enabled in `tauri.conf.json` under
 * `app.security.assetProtocol`.
 */

const REMOTE_OR_DATA = /^(https?:|data:|blob:|asset:|tauri:)/i;

/**
 * Returns true if `src` is a non-filesystem reference that should be
 * passed through unchanged (HTTP, data URI, blob URL, or an asset URL
 * that's already been converted).
 */
export function isRemoteOrDataUrl(src: string): boolean {
  return REMOTE_OR_DATA.test(src);
}

/**
 * Returns true for POSIX absolute paths (`/foo`) and Windows drive
 * paths (`C:\foo`, `D:/bar`). Doesn't validate that the path exists —
 * just that it's absolute.
 */
export function isAbsoluteFsPath(src: string): boolean {
  return src.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(src);
}

/**
 * Returns the directory portion of a path (everything before the last
 * `/` or `\`). Returns `""` if the path has no separator.
 */
export function dirname(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return idx >= 0 ? path.slice(0, idx) : "";
}

/**
 * Joins a directory and a (possibly nested) relative path. Strips a
 * single leading `./` from the relative chunk; multi-segment relatives
 * like `../foo` are passed through unchanged — the OS resolves them
 * when the asset protocol opens the file. Picks `\` only when `dir`
 * looks Windows-shaped (contains `\` and no `/`).
 */
export function joinPath(dir: string, rel: string): string {
  const cleaned = rel.replace(/^\.\//, "");
  if (!dir) return cleaned;
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return `${dir}${sep}${cleaned}`;
}

/**
 * Resolve an image `src` from a markdown document into a URL the
 * webview can load.
 *
 * - Remote / data / asset URLs pass through unchanged.
 * - Absolute filesystem paths go through `convertFileSrc()`.
 * - Relative paths are joined against the markdown file's directory
 *   (when known) and then converted.
 * - If the path is relative and we have no markdown file context,
 *   the original `src` is returned so the browser's own resolution
 *   can take a shot at it (e.g. `data:` URLs in synthetic content).
 */
export function resolveAssetSrc(
  src: string | undefined,
  baseFilePath: string | null | undefined,
): string | undefined {
  if (!src) return src;
  if (isRemoteOrDataUrl(src)) return src;
  if (isAbsoluteFsPath(src)) return convertFileSrc(src);
  if (baseFilePath) {
    return convertFileSrc(joinPath(dirname(baseFilePath), src));
  }
  return src;
}
