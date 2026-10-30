/**
 * Pure helpers behind the Add project sheet: where new projects go, what a
 * pasted repository becomes, and which folder names are acceptable.
 */

/** Parent folder for new and cloned projects until the user picks one.
 *  Before the home dir is known the tilde form is used; the backend
 *  expands it. */
export function defaultProjectsLocation(homeDir: string | null): string {
  return homeDir ? joinPath(homeDir, "Projects") : "~/Projects";
}

/** Append one folder name to `base`, keeping the base's own separator so
 *  Windows paths stay Windows paths. */
export function joinPath(base: string, name: string): string {
  const sep = base.includes("\\") && !base.includes("/") ? "\\" : "/";
  return `${base.replace(/[\\/]+$/, "")}${sep}${name}`;
}

/** Why `name` can't be a project folder, or null when it can. */
export function projectNameError(name: string): string | null {
  if (!name.trim()) return "Enter a name for the project folder.";
  if (name === "." || name === ".." || /[\\/]/.test(name)) {
    return "Use a single folder name, without slashes.";
  }
  return null;
}

/** GitHub `owner/repo`: an owner (letters, digits, hyphens, so never a
 *  dotted host) and a repo name, nothing else. Anything with a colon, a
 *  leading dot or slash, or more segments is a URL, an SSH alias or a path,
 *  and belongs to git as typed. */
const GITHUB_SHORTHAND = /^[A-Za-z0-9][A-Za-z0-9-]*\/[\w-][\w.-]*$/;
/** The same `owner/repo` after a literal, scheme-less `github.com/`. */
const GITHUB_HOST_PATH = /^github\.com\/([A-Za-z0-9][A-Za-z0-9-]*\/[\w-][\w.-]*)$/i;

/**
 * Turn what the user typed into the address handed to `git clone`, or null
 * when nothing was typed. Only GitHub `owner/repo` shorthand and a
 * scheme-less `github.com/owner/repo` are rewritten; every other address
 * (other hosts, SSH host aliases, local paths, `file://`, `insteadOf`
 * prefixes such as `corp.example/team/repo`) goes to git unchanged and git
 * says whether it is a repository.
 */
export function normalizeCloneUrl(input: string): string | null {
  const value = input.trim();
  if (!value) return null;
  const repo = GITHUB_SHORTHAND.test(value) ? value : GITHUB_HOST_PATH.exec(value)?.[1];
  if (repo) return `https://github.com/${repo.replace(/\.git$/i, "")}.git`;
  return value;
}

/** The folder a clone lands in unless renamed: the repository's last path
 *  segment, without `.git`. */
export function cloneFolderName(input: string): string {
  const last = input
    .trim()
    .replace(/[\\/]+$/, "")
    .replace(/\.git$/i, "")
    .split(/[/:\\]/)
    .pop();
  return (last ?? "").replace(/[^\w.-]/g, "");
}
