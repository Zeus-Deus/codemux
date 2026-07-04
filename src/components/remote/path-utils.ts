/**
 * Pure path helpers for the web remote path browser.
 *
 * The remote host may run any OS, so these never touch the browser's own
 * (POSIX) notion of paths — they operate on the raw host path strings that
 * `get_home_dir` / `list_directory` return, inferring the separator from
 * the string itself. Kept side-effect-free so the navigation logic is
 * unit-testable without a DOM.
 */

/** Infer the host path separator from a path string: backslash only when
 *  the string clearly looks like a Windows path (has `\`, no `/`), else the
 *  POSIX `/`. */
export function detectSeparator(path: string): "/" | "\\" {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

/** True when `path` is a filesystem root — `/` (POSIX) or a bare drive like
 *  `C:\` / `C:` (Windows). Roots have no parent to navigate up to. */
export function isRootPath(path: string): boolean {
  const p = path.trim();
  if (p === "/" || p === "") return true;
  // Windows drive root: "C:", "C:\", "C:/".
  if (/^[a-zA-Z]:[\\/]?$/.test(p)) return true;
  return false;
}

/** Strip a single trailing separator (but never the lone root `/`). */
function stripTrailingSep(path: string, sep: "/" | "\\"): string {
  if (path.length > 1 && path.endsWith(sep)) {
    return path.slice(0, -1);
  }
  return path;
}

/** The parent directory of `path`, or `path` itself when already at a root.
 *  Separator-aware so it works for both POSIX and Windows host paths. */
export function parentPath(path: string): string {
  const sep = detectSeparator(path);
  const trimmedWs = path.trim();
  // Check root before stripping so a drive root ("C:\") keeps its trailing
  // separator instead of collapsing to a bare "C:".
  if (isRootPath(trimmedWs)) return trimmedWs;

  const trimmed = stripTrailingSep(trimmedWs, sep);
  const idx = trimmed.lastIndexOf(sep);
  if (idx < 0) return trimmed;
  if (idx === 0) return sep; // POSIX child of root, e.g. "/foo" → "/"

  const parent = trimmed.slice(0, idx);
  // "C:\foo" → "C:\" (keep the drive-root separator so it reads as a root).
  if (/^[a-zA-Z]:$/.test(parent)) return parent + sep;
  return parent;
}

export interface Breadcrumb {
  /** Display label for the segment (drive/root or directory name). */
  name: string;
  /** Absolute path this crumb navigates to. */
  path: string;
}

/**
 * Break an absolute host path into clickable breadcrumb segments, each
 * carrying the absolute path to navigate to when clicked. The first crumb is
 * the root (`/` or the drive); subsequent crumbs are the nested directories.
 */
export function pathBreadcrumbs(path: string): Breadcrumb[] {
  const sep = detectSeparator(path);
  const trimmed = stripTrailingSep(path.trim(), sep);

  if (trimmed === "" || trimmed === sep) {
    return [{ name: sep, path: sep }];
  }

  // Windows: leading "C:" is the root crumb; the rest are nested.
  const driveMatch = /^([a-zA-Z]:)(.*)$/.exec(trimmed);
  if (driveMatch && sep === "\\") {
    const drive = driveMatch[1];
    const rest = driveMatch[2].replace(/^\\+/, "");
    const crumbs: Breadcrumb[] = [{ name: drive, path: drive + sep }];
    let acc = drive;
    for (const part of rest.split("\\").filter(Boolean)) {
      acc = `${acc}${sep}${part}`;
      crumbs.push({ name: part, path: acc });
    }
    return crumbs;
  }

  // POSIX: root crumb + each nested directory.
  const crumbs: Breadcrumb[] = [{ name: sep, path: sep }];
  let acc = "";
  for (const part of trimmed.split(sep).filter(Boolean)) {
    acc = `${acc}${sep}${part}`;
    crumbs.push({ name: part, path: acc });
  }
  return crumbs;
}
