/**
 * Unsaved edits to isolated plugins' settings, held in memory until Save or
 * Revert. They outlive the form, so picking another plugin, a catalogue
 * push or leaving Settings never loses one, and nothing applies by itself.
 * A draft belongs to one set of files: another digest may declare other
 * settings, so it is dropped then. Nothing here is written to disk.
 */
import { create } from "zustand";
import type { PluginInstallation, PluginSettingValue } from "./contract";
import { buildPluginSettingsPatch, pluginSettingsDraft, type PluginSettingDraft } from "./model";

export interface PluginSettingsDraftEntry {
  readonly digest: string;
  /** The stored values the edit started from; Save sends only what differs from these. */
  readonly base: Readonly<Record<string, PluginSettingValue>>;
  readonly draft: PluginSettingDraft;
  readonly clearedSecrets: ReadonlySet<string>;
}

export const usePluginSettingsDrafts = create<{
  readonly drafts: Readonly<Record<string, PluginSettingsDraftEntry>>;
  /** Installations with a Save in flight. Shared, so a form remounted by
   *  switching plugins still waits for its Save instead of taking new edits. */
  readonly saving: Readonly<Record<string, true>>;
}>(() => ({ drafts: {}, saving: {} }));

const entryFor = (
  drafts: Readonly<Record<string, PluginSettingsDraftEntry>>,
  installation: PluginInstallation,
): PluginSettingsDraftEntry | null => {
  const entry = drafts[installation.installationId];
  return entry !== undefined && entry.digest === installation.digest ? entry : null;
};

/** The installation's unsaved edit, if any. */
export function usePluginSettingsDraft(installation: PluginInstallation): PluginSettingsDraftEntry | null {
  return usePluginSettingsDrafts((state) => entryFor(state.drafts, installation));
}

/** What the form shows: the unsaved edit, or the stored values while nothing is edited. */
export function pluginSettingsView(installation: PluginInstallation, entry: PluginSettingsDraftEntry | null) {
  return entry ?? {
    digest: installation.digest,
    base: installation.settings,
    draft: pluginSettingsDraft(installation),
    clearedSecrets: new Set<string>(),
  };
}

/**
 * Records an edit. One that returns every field to its stored value is no
 * edit at all, so the draft goes and the form follows the host again.
 */
export function editPluginSettings(
  installation: PluginInstallation,
  next: Pick<PluginSettingsDraftEntry, "draft" | "clearedSecrets">,
): void {
  const current = entryFor(usePluginSettingsDrafts.getState().drafts, installation);
  const base = current?.base ?? installation.settings;
  const patch = buildPluginSettingsPatch(installation.manifest.settings, base, next.draft, next.clearedSecrets);
  const clean = patch.ok && Object.keys(patch.values).length === 0;
  setDraft(
    installation.installationId,
    clean ? null : { digest: installation.digest, base, draft: next.draft, clearedSecrets: next.clearedSecrets },
  );
}

/** Whether a Save is in flight for the installation. */
export function usePluginSettingsSaving(installationId: string): boolean {
  return usePluginSettingsDrafts((state) => state.saving[installationId] === true);
}

export function setPluginSettingsSaving(installationId: string, saving: boolean): void {
  usePluginSettingsDrafts.setState((state) => {
    if (saving === (state.saving[installationId] === true)) return state;
    const next = { ...state.saving };
    if (saving) next[installationId] = true;
    else delete next[installationId];
    return { saving: next };
  });
}

/**
 * Drops the installation's draft. Given the draft a Save submitted, drops it
 * only while it is still that draft: an edit made since is newer and stays.
 */
export function discardPluginSettings(installationId: string, submitted?: PluginSettingsDraftEntry | null): void {
  if (submitted !== undefined && usePluginSettingsDrafts.getState().drafts[installationId] !== submitted) return;
  setDraft(installationId, null);
}

/** Drops drafts for plugins that are gone or whose files changed. */
export function prunePluginSettingsDrafts(installations: ReadonlyArray<PluginInstallation>): void {
  const { drafts } = usePluginSettingsDrafts.getState();
  const live = new Map(installations.map((installation) => [installation.installationId, installation.digest]));
  const stale = Object.entries(drafts).filter(([installationId, entry]) => live.get(installationId) !== entry.digest);
  if (stale.length === 0) return;
  const next = { ...drafts };
  for (const [installationId] of stale) delete next[installationId];
  usePluginSettingsDrafts.setState({ drafts: next });
}

function setDraft(installationId: string, entry: PluginSettingsDraftEntry | null): void {
  usePluginSettingsDrafts.setState(({ drafts }) => {
    if (entry !== null) return { drafts: { ...drafts, [installationId]: entry } };
    if (!(installationId in drafts)) return { drafts };
    const next = { ...drafts };
    delete next[installationId];
    return { drafts: next };
  });
}
