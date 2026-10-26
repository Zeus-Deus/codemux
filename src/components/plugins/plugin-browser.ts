/**
 * The Settings plugin list: order, search, the attention filter and the
 * hints that tell same-named plugins apart. Pure, so the list stays
 * predictable at any size.
 */
import type { PluginInstallation } from "./contract";
import {
  pluginAttention,
  pluginReviewSubject,
  pluginSourceLabel,
  type PluginReviewRevision,
  type PluginStatusPresentation,
} from "./model";

const STAGED_LABELS: Record<Exclude<PluginReviewRevision, "install">, (version: string) => string> = {
  update: (version) => `Update ${version} ready`,
  "local-repair": () => "Local fix ready",
  previous: (version) => `Previous v${version} ready`,
};

/** What a staged revision waiting for review is, in a few words; null when none waits. */
export function pluginStagedLabel(installation: PluginInstallation): string | null {
  const pending = installation.pendingUpdate;
  if (pending === undefined) return null;
  const revision = pluginReviewSubject(installation).revision;
  return revision === "install" ? null : STAGED_LABELS[revision](pending.manifest.version);
}

/**
 * Something the user can act on: a review, a staged revision, or a reported
 * problem. A plugin that is off and merely can't run here is not counted;
 * its status already says so.
 */
export function pluginNeedsAttention(installation: PluginInstallation, status: PluginStatusPresentation): boolean {
  if (status.primary !== "none" || installation.pendingUpdate !== undefined) return true;
  return pluginAttention(installation)?.notify === true;
}

/** `needle` is already trimmed and lower-cased; empty matches everything. */
export function pluginMatchesQuery(installation: PluginInstallation, needle: string): boolean {
  if (needle === "") return true;
  const { name, id, description } = installation.manifest;
  return [name, id, description ?? "", pluginSourceLabel(installation.source)].join("\n").toLowerCase().includes(needle);
}

/** By name, then installation id: stable through status changes, so rows never jump. */
export function sortPluginInstallations(
  installations: ReadonlyArray<PluginInstallation>,
): ReadonlyArray<PluginInstallation> {
  return [...installations].sort(
    (left, right) =>
      left.manifest.name.localeCompare(right.manifest.name, undefined, { sensitivity: "base" }) ||
      (left.installationId < right.installationId ? -1 : left.installationId > right.installationId ? 1 : 0),
  );
}

/**
 * For plugins that share a name, the identifier that tells them apart: the
 * manifest id, or the source when two installs share that too. Plugins with
 * a unique name get none.
 */
export function pluginDuplicateHints(installations: ReadonlyArray<PluginInstallation>): ReadonlyMap<string, string> {
  const byName = new Map<string, PluginInstallation[]>();
  for (const installation of installations) {
    const key = installation.manifest.name.trim().toLowerCase();
    byName.set(key, [...(byName.get(key) ?? []), installation]);
  }
  const hints = new Map<string, string>();
  for (const group of byName.values()) {
    if (group.length < 2) continue;
    for (const installation of group) {
      const sharedId = group.some(
        (other) => other !== installation && other.manifest.id === installation.manifest.id,
      );
      hints.set(installation.installationId, sharedId ? pluginSourceLabel(installation.source) : installation.manifest.id);
    }
  }
  return hints;
}

/** The row's one status line: state, then what waits, then the problem's title. */
export function pluginRowSummary(
  installation: PluginInstallation,
  status: PluginStatusPresentation,
  options: { readonly unsavedSettings?: boolean } = {},
): string {
  const attention = pluginAttention(installation);
  return [
    status.label,
    pluginStagedLabel(installation),
    attention && attention.kind !== "read-only" && attention.notify ? attention.title : null,
    options.unsavedSettings ? "Unsaved settings" : null,
  ]
    .filter(Boolean)
    .join(" · ");
}
