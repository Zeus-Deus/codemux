/** GitHub release page for `version`: the notes, plus manual downloads. */
export function releasePageUrl(version: string | null): string {
  const base = "https://github.com/Zeus-Deus/codemux/releases";
  return version ? `${base}/tag/v${version}` : `${base}/latest`;
}

/**
 * The command that updates a package-manager install, when Codemux knows it.
 * Only pacman installs are recognised: the AUR package is the one channel
 * besides AppImage and NSIS that the release process publishes to.
 */
export function manualUpdateCommand(packageFormat: string | null): string | null {
  return packageFormat === "pacman" ? "yay -S codemux-bin" : null;
}

/** Turn whatever the updater threw into one line a user can read. */
export function describeUpdateError(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error ?? "");
  const message = raw.trim();
  return message || "The updater stopped without giving a reason.";
}
