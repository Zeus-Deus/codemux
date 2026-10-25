import { useId, useState, type ReactNode } from "react";
import { Info, TriangleAlert } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";
import type {
  PluginEventBacklogPolicy,
  PluginGrants,
  PluginInstallation,
} from "./contract";
import {
  describePluginCapabilities,
  initialPluginGrants,
  isPluginRecordReadOnly,
  isPluginUnavailable,
  parsePluginSourceInput,
  pluginApprovalBacklogStep,
  pluginBacklogDecision,
  pluginCurrentRevisionNote,
  pluginDisplayVersion,
  pluginFailure,
  pluginHasUnsupportedRequirement,
  pluginNeedsProjects,
  pluginRecordIssueMessage,
  pluginRecoveryDetails,
  pluginRemovalNote,
  pluginReviewSubject,
  pluginSourceLabel,
  type PluginReviewRevision,
} from "./model";
import {
  platformBacklog,
  platformConsent,
  platformDiscardPendingUpdate,
  platformInstall,
  platformRemove,
  platformSetEnabled,
} from "./platform-adapter";
import { usePluginKnownProjects } from "./known-projects";
import { discardPluginSettings, usePluginSettingsDraft } from "./plugin-settings-drafts";

const formatTime = (iso: string) => {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
};

function Notice({ tone, children }: { tone: "info" | "warning"; children: ReactNode }) {
  const Icon = tone === "warning" ? TriangleAlert : Info;
  return (
    <div
      role={tone === "warning" ? "alert" : "status"}
      className={cn(
        "flex items-start gap-2 rounded-md border px-3 py-2 text-body-sm",
        tone === "warning" ? "border-status-attention/40 text-foreground" : "border-hairline text-muted-foreground",
      )}
    >
      <Icon aria-hidden className={cn("mt-0.5 size-3.5 shrink-0", tone === "warning" && "text-status-attention")} />
      <div className="min-w-0">{children}</div>
    </div>
  );
}

function FactList({ rows }: { rows: ReadonlyArray<{ label: string; value: string; code?: boolean }> }) {
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-body-sm">
      {rows.map((row) => (
        <div key={row.label} className="contents">
          <dt className="text-muted-foreground">{row.label}</dt>
          <dd className={row.code ? "break-all font-mono text-label leading-5" : "break-words"}>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}

const failureText = (error: unknown) => pluginFailure(error).message;

// ── install ───────────────────────────────────────────────────────────

/** Copies the plugin and reads its manifest. Nothing runs until it is reviewed. */
export function InstallPlatformPluginDialog({
  onClose,
  onInstalled,
}: {
  onClose: () => void;
  onInstalled: (installation: PluginInstallation) => void;
}) {
  const [source, setSource] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const id = useId();
  const submit = async () => {
    const parsed = parsePluginSourceInput(source);
    if (!parsed.ok) {
      setError(parsed.message);
      return;
    }
    setInstalling(true);
    setError(null);
    try {
      onInstalled(await platformInstall(parsed.source));
    } catch (cause) {
      setError(failureText(cause));
    } finally {
      setInstalling(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !installing && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <form
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Add isolated plugin</DialogTitle>
            <DialogDescription>
              Codemux copies the plugin and reads its codemux-plugin.json. Nothing runs until you review and turn it
              on. Only add plugins you trust: they run as your user.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-1.5">
            <Label htmlFor={`${id}-source`}>Source</Label>
            <Input
              id={`${id}-source`}
              autoFocus
              spellCheck={false}
              placeholder="/path/to/plugin or https://github.com/owner/plugin.git#v1.0.0"
              value={source}
              onChange={(event) => {
                setSource(event.target.value);
                setError(null);
              }}
            />
            {error ? (
              <p className="text-body-sm text-destructive" role="alert">
                {error}
              </p>
            ) : (
              <p className="text-body-sm text-muted-foreground">
                A folder on this computer, or an https Git URL. Add #tag or #commit to pin a version.
              </p>
            )}
          </div>
          <DialogFooter>
            <Button type="button" variant="ghost" size="sm" disabled={installing} onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={installing || source.trim() === ""}>
              {installing ? "Adding…" : "Add and review"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ── missed runs ───────────────────────────────────────────────────────

export type BacklogOutcome = { proceed: false } | { proceed: true; eventBacklog?: PluginEventBacklogPolicy };
/** Nothing to ask: proceed without a missed-runs choice. */
export const PROCEED: BacklogOutcome = { proceed: true };

/**
 * Before a plugin starts observing again, asks the host what it missed.
 * Missed runs are delivered or skipped only by the user's explicit choice.
 */
export function usePluginEventBacklog(recoverySupported: boolean) {
  const [prompt, setPrompt] = useState<{
    name: string;
    count: number;
    throughSequence: number;
    settle: (choice: PluginEventBacklogPolicy | null) => void;
  } | null>(null);
  const resolveBacklog = async (installation: PluginInstallation): Promise<BacklogOutcome> => {
    if (!recoverySupported) return { proceed: true };
    let backlog;
    try {
      backlog = await platformBacklog(installation.installationId);
    } catch (cause) {
      throw new Error(`Couldn't check what ${installation.manifest.name} missed: ${failureText(cause)}`);
    }
    const decision = pluginBacklogDecision(backlog);
    if (decision.kind === "none") return { proceed: true };
    if (decision.kind === "resolved") return { proceed: true, eventBacklog: decision.eventBacklog };
    const choice = await new Promise<PluginEventBacklogPolicy | null>((settle) =>
      setPrompt({ name: installation.manifest.name, count: decision.count, throughSequence: decision.throughSequence, settle }),
    );
    return choice === null ? { proceed: false } : { proceed: true, eventBacklog: choice };
  };
  const finish = (choice: PluginEventBacklogPolicy | null) => {
    prompt?.settle(choice);
    setPrompt(null);
  };
  const runs = prompt ? (prompt.count === 1 ? "1 run" : `${prompt.count} runs`) : "";
  const backlogDialog = prompt ? (
    <Dialog open onOpenChange={(open) => !open && finish(null)}>
      <DialogContent className="sm:max-w-md" data-plugin-backlog-dialog>
        <DialogHeader>
          <DialogTitle>
            {prompt.name} missed {runs}
          </DialogTitle>
          <DialogDescription>
            {runs} finished in your selected projects while {prompt.name} wasn't receiving events. Delivering them
            lets it act on each one now, which may send messages or start work. Skipping starts it from new runs only.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="ghost" size="sm" onClick={() => finish(null)}>
            Cancel
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => finish({ policy: "skip", throughSequence: prompt.throughSequence })}
          >
            Skip missed runs
          </Button>
          <Button size="sm" onClick={() => finish({ policy: "deliver", throughSequence: prompt.throughSequence })}>
            Deliver {runs}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  ) : null;
  return { resolveBacklog, backlogDialog };
}

// ── review ────────────────────────────────────────────────────────────

const REVIEW_TITLES: Record<PluginReviewRevision, string> = {
  install: "Review",
  update: "Update",
  "local-repair": "Use local fix for",
  previous: "Use previous version of",
};

const SETTING_CHANGE_LABELS = {
  added: "added",
  removed: "no longer used; its value stays with the previous version",
  "type-changed": "changes type; the stored value may reset",
} as const;

function reviewFooterLabels(revision: PluginReviewRevision, runningLocalFix: boolean) {
  switch (revision) {
    case "install":
      return { keep: "Not now", discard: "Discard", approve: "Approve and turn on" };
    case "update":
      return runningLocalFix
        ? { keep: "Keep local fix", discard: "Discard update", approve: "Switch to publisher version" }
        : { keep: "Keep current version", discard: "Discard update", approve: "Approve update" };
    case "local-repair":
      return { keep: "Keep current version", discard: "Discard staged fix", approve: "Use local fix" };
    case "previous":
      return { keep: "Keep current version", discard: "Discard", approve: "Use previous version" };
  }
}

/**
 * Projects a grant can name: every project Codemux knows (open and archived
 * workspaces), plus any already granted that it no longer knows, so a grant
 * can still be withdrawn. Nothing is guessed from paths.
 */
function useGrantableProjects(granted: ReadonlyArray<string>) {
  const known = usePluginKnownProjects();
  const projects: { id: string; title: string; detail: string }[] = known.map((project) => ({
    id: project.id,
    title: project.title,
    detail: project.archivedOnly ? `${project.root} · archived` : project.root,
  }));
  const ids = new Set(projects.map((project) => project.id));
  for (const id of granted) if (!ids.has(id)) projects.push({ id, title: "Unknown project", detail: id });
  return projects;
}

/**
 * What the user approves: the exact fingerprint shown, the permissions they
 * tick, and the projects those apply to. The host refuses an approval whose
 * fingerprint no longer matches the staged files.
 */
export function ReviewPlatformPluginDialog({
  installation,
  recoverySupported,
  onClose,
}: {
  installation: PluginInstallation;
  recoverySupported: boolean;
  onClose: () => void;
}) {
  const subject = pluginReviewSubject(installation);
  const { manifest } = subject;
  const id = useId();
  const isUpdate = subject.kind === "update";
  const capabilities = describePluginCapabilities(manifest);
  const runningLocalFix = installation.recovery?.activeSource === "local-repair";
  const currentRevisionNote = pluginCurrentRevisionNote(installation);
  const unsupported = capabilities.filter((entry) => !entry.known && !entry.optional).map((entry) => entry.capability);
  const labels = reviewFooterLabels(subject.revision, runningLocalFix);
  // The running version's availability says nothing about a staged update.
  const incompatible = (!isUpdate && isPluginUnavailable(installation)) || pluginHasUnsupportedRequirement(manifest);
  const [grants, setGrants] = useState<PluginGrants>(() => initialPluginGrants(installation));
  const projects = useGrantableProjects(grants.projectIds);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { resolveBacklog, backlogDialog } = usePluginEventBacklog(recoverySupported);
  const changed =
    !isUpdate && installation.consentedDigest !== null && installation.consentedDigest !== installation.digest;
  const projectScopedGranted = capabilities.some(
    (entry) => entry.known && entry.projectScoped && grants.capabilities.includes(entry.capability),
  );
  const toggleCapability = (capability: string, granted: boolean) =>
    setGrants((current) => ({
      ...current,
      capabilities: granted
        ? [...current.capabilities, capability]
        : current.capabilities.filter((entry) => entry !== capability),
    }));
  const toggleProject = (projectId: string, granted: boolean) =>
    setGrants((current) => ({
      ...current,
      projectIds: granted ? [...current.projectIds, projectId] : current.projectIds.filter((entry) => entry !== projectId),
    }));
  const discard = async () => {
    setSaving(true);
    setError(null);
    try {
      await platformDiscardPendingUpdate(installation.installationId);
      onClose();
    } catch (cause) {
      setError(`Couldn't discard: ${failureText(cause)}`);
    } finally {
      setSaving(false);
    }
  };
  const approve = async () => {
    setSaving(true);
    setError(null);
    try {
      // Asked before anything is saved, so cancelling leaves the plugin as it was.
      const backlogStep = pluginApprovalBacklogStep(installation);
      const backlog = backlogStep === null ? PROCEED : await resolveBacklog(installation);
      if (!backlog.proceed) return;
      const eventBacklog = backlog.eventBacklog ? { eventBacklog: backlog.eventBacklog } : {};
      const consented = await platformConsent({
        installationId: installation.installationId,
        digest: subject.digest,
        grants,
        ...(backlogStep === "consent" ? eventBacklog : {}),
      });
      // Approving an update keeps the plugin's on/off choice; a first approval turns it on.
      if (!isUpdate && !consented.enabled) {
        try {
          await platformSetEnabled({
            installationId: installation.installationId,
            enabled: true,
            ...(backlogStep === "enable" ? eventBacklog : {}),
          });
        } catch (cause) {
          setError(`Approved, but couldn't turn it on: ${failureText(cause)}`);
          return;
        }
      }
      onClose();
    } catch (cause) {
      setError(`Couldn't approve: ${failureText(cause)}`);
    } finally {
      setSaving(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !saving && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl" data-plugin-review={installation.installationId}>
        <DialogHeader>
          <DialogTitle>
            {REVIEW_TITLES[subject.revision]} {manifest.name}{" "}
            <span className="font-normal text-muted-foreground">v{manifest.version}</span>
          </DialogTitle>
          <DialogDescription>
            {manifest.description ?? "Choose what this plugin may do before any of its code runs."}
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-5">
          {subject.revision === "update" ? (
            <Notice tone="info">
              {runningLocalFix
                ? `This is the publisher's version ${manifest.version}. ${currentRevisionNote} After switching, your local fix stays available as the previous version.`
                : `${currentRevisionNote} Keeping the current version leaves the update waiting.`}
            </Notice>
          ) : null}
          {subject.revision === "local-repair" ? (
            <Notice tone="info">
              These files come from your repair folder. {currentRevisionNote} Settings, data and event position carry
              over, and Codemux keeps tracking the publisher's source so you can switch back later.
            </Notice>
          ) : null}
          {subject.revision === "previous" ? (
            <Notice tone="warning">
              Version {manifest.version} uses your current settings and data, not the data it had before. Codemux
              can't undo what version {installation.manifest.version} changed, inside or outside Codemux.
            </Notice>
          ) : null}
          {changed ? (
            <Notice tone="warning">
              The plugin's files changed since you approved them. It stays stopped until you approve this version.
            </Notice>
          ) : null}
          <FactList
            rows={[
              { label: "Identifier", value: manifest.id, code: true },
              { label: "Source", value: pluginSourceLabel(subject.source) },
              { label: "Fingerprint", value: subject.digest, code: true },
            ]}
          />
          <section className="grid gap-2">
            <h3 className="text-body font-medium">Permissions</h3>
            {capabilities.length === 0 ? (
              <p className="text-body-sm text-muted-foreground">This plugin asks for no permissions.</p>
            ) : (
              <ul className="grid gap-2">
                {capabilities.map((entry, index) => (
                  <li key={entry.capability}>
                    <label className="flex min-h-11 items-start gap-2.5">
                      <Checkbox
                        className="mt-0.5 min-h-0! after:-inset-y-3.5 after:-inset-x-3.5"
                        aria-labelledby={`${id}-grant-${index}`}
                        aria-describedby={`${id}-grant-${index}-detail`}
                        disabled={!entry.known || saving}
                        checked={entry.known && grants.capabilities.includes(entry.capability)}
                        onCheckedChange={(checked) => entry.known && toggleCapability(entry.capability, checked === true)}
                      />
                      <span className="min-w-0 text-body-sm">
                        <span id={`${id}-grant-${index}`} className={entry.known ? "" : "text-destructive"}>
                          {entry.title}
                        </span>
                        {subject.newCapabilities.includes(entry.capability) ? (
                          <Badge variant="secondary" className="ms-2">
                            New
                          </Badge>
                        ) : null}
                        {entry.optional ? (
                          <Badge variant="outline" className="ms-2">
                            Optional
                          </Badge>
                        ) : null}
                        <span id={`${id}-grant-${index}-detail`} className="block text-label text-muted-foreground">
                          {entry.detail}
                        </span>
                      </span>
                    </label>
                  </li>
                ))}
              </ul>
            )}
          </section>
          {subject.removedCapabilities.length > 0 || subject.settingChanges.length > 0 ? (
            <section className="grid gap-1">
              <h3 className="text-body font-medium">Changes from v{installation.manifest.version}</h3>
              <ul className="grid gap-0.5 text-body-sm text-muted-foreground">
                {subject.removedCapabilities.map((capability) => (
                  <li key={`cap:${capability}`}>No longer asks for {capability}</li>
                ))}
                {subject.settingChanges.map((change) => (
                  <li key={`setting:${change.key}`}>
                    {change.label} setting {SETTING_CHANGE_LABELS[change.change]}
                  </li>
                ))}
              </ul>
            </section>
          ) : null}
          {pluginNeedsProjects(manifest) ? (
            <section className="grid gap-2">
              <h3 className="text-body font-medium">Projects</h3>
              <p className="text-label text-muted-foreground">
                Chat and background permissions apply only to the projects you pick.
              </p>
              {projects.length === 0 ? (
                <p className="text-body-sm text-muted-foreground">Codemux doesn't know any projects yet.</p>
              ) : (
                <ul className="grid max-h-48 gap-1.5 overflow-y-auto">
                  {projects.map((project, index) => (
                    <li key={project.id}>
                      <label className="flex min-h-11 items-center gap-2.5">
                        <Checkbox
                          className="min-h-0! after:-inset-y-3.5 after:-inset-x-3.5"
                          // Indexed: project ids are paths, which may hold spaces.
                          aria-labelledby={`${id}-project-${index}`}
                          disabled={!projectScopedGranted || saving}
                          checked={grants.projectIds.includes(project.id)}
                          onCheckedChange={(checked) => toggleProject(project.id, checked === true)}
                        />
                        <span className="min-w-0 text-body-sm">
                          <span id={`${id}-project-${index}`} className="block truncate">
                            {project.title}
                          </span>
                          <span className="block truncate font-mono text-label text-muted-foreground">{project.detail}</span>
                        </span>
                      </label>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : null}
          {manifest.actions.length > 0 ||
          manifest.views.length > 0 ||
          (manifest.sidebarViews ?? []).length > 0 ||
          (manifest.nativeTrees ?? []).length > 0 ? (
            <section className="grid gap-1">
              <h3 className="text-body font-medium">Adds</h3>
              <ul className="grid gap-0.5 text-body-sm text-muted-foreground">
                {manifest.actions.map((action) => (
                  <li key={`a:${action.id}`}>
                    {action.title}
                    {action.agentCallable ? " (agents can call it)" : ""}
                  </li>
                ))}
                {manifest.views.map((view) => (
                  <li key={`v:${view.id}`}>{view.title} view</li>
                ))}
                {(manifest.sidebarViews ?? []).map((view) => (
                  <li key={`s:${view.id}`}>{view.title} sidebar layout (off until you choose it)</li>
                ))}
                {(manifest.nativeTrees ?? []).map((tree) => (
                  <li key={`t:${tree.id}`}>{tree.title} sidebar layout (off until you choose it)</li>
                ))}
              </ul>
            </section>
          ) : null}
          {incompatible ? (
            <p className="text-body-sm text-destructive" role="alert">
              {unsupported.length > 0
                ? `Needs ${unsupported.join(", ")}, which this version of Codemux does not support, so it cannot be turned on.`
                : (installation.error ?? "This plugin needs something this version of Codemux does not support.")}
            </p>
          ) : null}
          {error ? (
            <p className="text-body-sm text-destructive" role="alert">
              {error}
            </p>
          ) : null}
          <p className="text-label text-muted-foreground">
            Plugins are trusted code that runs as your user. These permissions limit what it can do through Codemux,
            not its own access to your files or the network.
          </p>
        </div>
        <DialogFooter>
          <Button variant="ghost" size="sm" disabled={saving} onClick={onClose}>
            {isUpdate ? labels.keep : incompatible ? "Close" : "Not now"}
          </Button>
          {isUpdate ? (
            <Button variant="outline" size="sm" disabled={saving} onClick={() => void discard()}>
              {labels.discard}
            </Button>
          ) : null}
          {incompatible ? null : (
            <Button size="sm" disabled={saving} onClick={() => void approve()}>
              {saving ? "Saving…" : labels.approve}
            </Button>
          )}
        </DialogFooter>
        {backlogDialog}
      </DialogContent>
    </Dialog>
  );
}

// ── remove ────────────────────────────────────────────────────────────

/** Keeps settings and data by default, so reinstalling picks up where it left off. */
export function RemovePlatformPluginDialog({
  installation,
  onClose,
  onRemoved = onClose,
}: {
  installation: PluginInstallation;
  onClose: () => void;
  onRemoved?: () => void;
}) {
  const unsavedSettings = usePluginSettingsDraft(installation) !== null;
  const [deleteData, setDeleteData] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirm = async () => {
    setRemoving(true);
    setError(null);
    try {
      await platformRemove(installation.installationId, deleteData);
      discardPluginSettings(installation.installationId);
      onRemoved();
    } catch (cause) {
      setError(failureText(cause));
    } finally {
      setRemoving(false);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && !removing && onClose()}>
      <DialogContent className="sm:max-w-md" data-plugin-remove-dialog>
        <DialogHeader>
          <DialogTitle>Remove {installation.manifest.name}?</DialogTitle>
          <DialogDescription>
            The plugin stops and its files are removed. Its settings and stored data are kept unless you delete them,
            so reinstalling picks up where it left off. {pluginRemovalNote(installation)}
            {unsavedSettings ? " Your unsaved settings changes are discarded." : ""}
          </DialogDescription>
        </DialogHeader>
        <label className="flex min-h-11 items-center gap-2.5 text-body-sm">
          <Checkbox
            className="min-h-0! after:-inset-y-3.5 after:-inset-x-3.5"
            checked={deleteData}
            aria-label="Also delete its settings and stored data"
            onCheckedChange={(checked) => setDeleteData(checked === true)}
          />
          Also delete its settings and stored data
        </label>
        {error ? (
          <p className="text-body-sm text-destructive" role="alert">
            {error}
          </p>
        ) : null}
        <DialogFooter>
          <Button variant="ghost" size="sm" disabled={removing} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="destructive" size="sm" disabled={removing} onClick={() => void confirm()}>
            {removing ? "Removing…" : deleteData ? "Remove and delete data" : "Remove"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ── details and previous version ──────────────────────────────────────

/** Why a plugin is in its state; versions as recorded, no guessed causes. */
export function PlatformPluginDetailsDialog({
  installation,
  onClose,
}: {
  installation: PluginInstallation;
  onClose: () => void;
}) {
  const recordIssue = pluginRecordIssueMessage(installation);
  const version = pluginDisplayVersion(installation);
  const rows = [
    ...pluginRecoveryDetails(installation, formatTime),
    ...(version === null ? [] : [{ label: "Version", value: `v${version}` }]),
    ...(isPluginRecordReadOnly(installation)
      ? []
      : [
          { label: "Source", value: pluginSourceLabel(installation.source), code: true },
          { label: "Fingerprint", value: installation.digest, code: true },
          { label: "Installation", value: installation.installationId, code: true },
        ]),
  ];
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-xl" data-plugin-details={installation.installationId}>
        <DialogHeader>
          <DialogTitle>{installation.manifest.name} details</DialogTitle>
          <DialogDescription>
            What Codemux recorded about this plugin. Versions are shown as they were when each event happened; Codemux
            does not guess which change caused a failure.
          </DialogDescription>
        </DialogHeader>
        {recordIssue ? <Notice tone="info">{recordIssue}</Notice> : null}
        <FactList rows={rows} />
        <p className="text-label text-muted-foreground">
          Plugins are trusted code that runs as your user with your access to files and the network. Permissions
          limit what they can do through Codemux; their views show in a sandboxed frame.
        </p>
        <DialogFooter>
          <Button size="sm" onClick={onClose}>
            Close
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Running the kept previous code is not a rollback: data, files and outside
 * effects stay as the current version left them. Staging it still goes
 * through the normal review.
 */
export function UsePreviousPlatformPluginDialog({
  installation,
  previousVersion,
  onConfirm,
  onClose,
}: {
  installation: PluginInstallation;
  previousVersion: string;
  onConfirm: () => Promise<void>;
  onClose: () => void;
}) {
  const [staging, setStaging] = useState(false);
  const { name, version } = installation.manifest;
  return (
    <Dialog open onOpenChange={(open) => !open && !staging && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            Use {name} v{previousVersion} with current data?
          </DialogTitle>
          <DialogDescription>
            Version {previousVersion} uses your current settings and data. Version {version} may have changed them, and
            Codemux can't undo that or anything either version did outside Codemux. If v{previousVersion} can't read the
            data, it should stop with an error.
          </DialogDescription>
        </DialogHeader>
        <p className="text-body-sm text-muted-foreground">You review it next. {pluginCurrentRevisionNote(installation)}</p>
        <DialogFooter>
          <Button variant="ghost" size="sm" disabled={staging} onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            disabled={staging}
            onClick={async () => {
              setStaging(true);
              try {
                await onConfirm();
              } finally {
                setStaging(false);
              }
            }}
          >
            {staging ? "Preparing…" : "Continue to review"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
