/**
 * Collapse `$HOME` to `~` so long worktree paths stay readable in narrow
 * surfaces (hover cards, the command palette, pickers).
 *
 * Only a real home-directory boundary counts: with home `/home/u`, a sibling
 * like `/home/u2/project` must stay absolute, so the character after the
 * prefix has to be a path separator (or the path IS home). Accepts both `/`
 * and `\` since remote workspaces can live on Windows hosts. Falls back to the
 * raw path when the home dir isn't known yet.
 */
export function shortenPath(path: string, homeDir: string | null): string {
  if (!homeDir) return path;
  // A reported home dir may carry a trailing separator ("/home/u/"); strip it
  // so the boundary check below sees the bare prefix.
  const home = homeDir.replace(/[/\\]+$/, "");
  if (home.length === 0 || !path.startsWith(home)) return path;
  const next = path[home.length];
  if (next === undefined) return "~"; // the path IS the home dir
  if (next !== "/" && next !== "\\") return path; // sibling prefix, not home
  return `~${path.slice(home.length)}`;
}
