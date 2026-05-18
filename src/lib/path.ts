/**
 * Cross-platform path helpers for UI labels.
 *
 * The backend hands us paths in their native form: POSIX paths on
 * Linux/macOS (`/home/user/proj`) and Windows paths with backslashes
 * (`C:\Users\user\proj`). UI code that needs the final folder name
 * must split on both separators, otherwise a Windows path returns
 * unchanged from `split("/").pop()` and we end up rendering the full
 * `C:\...` string instead of the project basename.
 */

const SEP = /[\\/]/;

/** Last non-empty segment of a path, or the original string if empty. */
export function basename(path: string): string {
  return path.split(SEP).filter(Boolean).pop() || path;
}

/** Last N path segments joined with `/` — used for disambiguating
 *  duplicate basenames in sidebar labels. */
export function tailSegments(path: string, n: number): string {
  const parts = path.split(SEP).filter(Boolean);
  return parts.slice(-n).join("/");
}
