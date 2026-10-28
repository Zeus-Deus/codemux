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

/**
 * Turn what the user typed into something `git clone` accepts, or null when
 * it isn't recognisably a repository. `owner/repo` is GitHub shorthand and a
 * scheme-less `host/owner/repo` gets https.
 */
export function normalizeCloneUrl(input: string): string | null {
  const value = input.trim();
  if (!value) return null;
  if (/^[\w.-]+\/[\w.-]+$/.test(value)) {
    return `https://github.com/${value.replace(/\.git$/i, "")}.git`;
  }
  if (/^(https?|ssh|git|file):\/\/\S+$/i.test(value)) return value;
  // scp-style ssh: git@github.com:owner/repo.git
  if (/^[\w.-]+@[\w.-]+:\S+$/.test(value)) return value;
  if (/^[\w-]+(\.[\w-]+)+\/\S+$/.test(value)) return `https://${value}`;
  return null;
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
