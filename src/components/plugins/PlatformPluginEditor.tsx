import { useId, useState, type ReactNode, type Ref } from "react";
import {
  Bot,
  ChevronRight,
  FileSearch,
  History,
  House,
  Info,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  SquareArrowOutUpRight,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapse } from "@/components/ui/collapse";
import { Switch } from "@/components/ui/switch";
import { DetailSection } from "@/components/settings/provider-parts";
import { copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { PluginInstallation } from "./contract";
import { usePluginKnownProjects } from "./known-projects";
import {
  describePluginCapabilities,
  isPluginActive,
  isPluginRecordReadOnly,
  isPluginUnavailable,
  pluginAttention,
  pluginCurrentRevisionNote,
  pluginDisplayVersion,
  pluginFailure,
  pluginInactiveReason,
  pluginNeedsProjects,
  pluginRecordIssueMessage,
  pluginRecoveryActions,
  pluginReviewSubject,
  pluginSourceLabel,
  type PluginReviewRevision,
  type PluginStatusPresentation,
  type PluginStatusTone,
} from "./model";
import { applyPluginIntents, capturePluginTarget, runPluginAction } from "./plugin-actions";
import {
  platformPrepareRepair,
  platformRefresh,
  platformResume,
  platformStageRepair,
} from "./platform-adapter";
import { COMPACT_SWITCH, PlatformPluginSettingsForm } from "./PlatformPluginSettingsForm";
import { usePluginEventBacklog } from "./PlatformPluginDialogs";
import { pluginStartPageFor, type PluginStartPage } from "./start-page";

export type PluginDialog =
  | { kind: "install" }
  | { kind: "review"; installationId: string; installed?: PluginInstallation }
  | { kind: "remove" | "details" | "use-previous"; installationId: string };

export const TONE_DOT: Record<PluginStatusTone, string> = {
  success: "bg-status-open",
  warning: "bg-status-attention",
  error: "bg-destructive",
  muted: "bg-muted-foreground/50",
};

const TONE_BADGE: Record<PluginStatusTone, string> = {
  success: "border-status-open/40 text-foreground",
  warning: "border-status-attention/50 text-foreground",
  error: "border-destructive/50 text-destructive",
  muted: "text-muted-foreground",
};

const REVIEW_LABELS: Record<PluginReviewRevision, string> = {
  install: "Review",
  update: "Review update",
  "local-repair": "Review local fix",
  previous: "Review previous version",
};

const PLACEMENT_LABELS: Record<string, string> = {
  palette: "Command palette",
  "thread-menu": "Chat menu",
  "message-selection": "Reply menu",
};

const failureText = (error: unknown) => pluginFailure(error).message;

/**
 * Repair with agent: the host copies the exact running files into a repair
 * folder; this opens an ordinary chat draft there holding the redacted
 * diagnostics. Nothing is sent, enabled or installed.
 */
async function openRepairDraft(installation: PluginInstallation): Promise<void> {
  const workspace = await platformPrepareRepair(installation.installationId);
  const flags = useFeatureFlags.getState();
  if (flags.enableAgentChat && flags.enableLazyWorkspaceCreation) {
    const store = useChatDraftStore.getState();
    const draft = store.getOrCreateProjectDraft(workspace.directory);
    // The agent must edit the folder itself, on this computer: staging reads
    // the folder, so a worktree or another device would hide the fix.
    store.updateDraftConfig(draft.draftId, { checkoutMode: "current", hostId: null });
    store.updateDraftInput(draft.draftId, workspace.draft);
    store.setActiveDraft(draft.draftId);
    useUIStore.getState().setShowSettings(false);
    return;
  }
  const copied = await copyToClipboard(workspace.draft);
  toast.info(`${installation.manifest.name} repair folder is ready`, {
    description: `${copied ? "The diagnostics are on your clipboard. " : ""}Open ${workspace.directory} with an agent to fix it.`,
  });
}

/** Restart, refresh and repair for one plugin, with the outcome toasts. */
function usePluginLifecycle(installation: PluginInstallation, recoverySupported: boolean, onReview: () => void) {
  const { manifest, installationId: id } = installation;
  const { resolveBacklog, backlogDialog } = usePluginEventBacklog(recoverySupported);
  const [busy, setBusy] = useState(false);
  const withBusy = async (title: string, run: () => Promise<void>) => {
    setBusy(true);
    try {
      await run();
    } catch (cause) {
      toast.error(title, { description: failureText(cause) });
    } finally {
      setBusy(false);
    }
  };
  return {
    busy,
    backlogDialog,
    resume: () =>
      withBusy(`Couldn't restart ${manifest.name}`, async () => {
        const backlog = await resolveBacklog(installation);
        if (!backlog.proceed) return;
        await platformResume({ installationId: id, ...(backlog.eventBacklog ? { eventBacklog: backlog.eventBacklog } : {}) });
      }),
    refresh: () =>
      withBusy(`Couldn't check ${manifest.name} for changes`, async () => {
        const result = await platformRefresh(id);
        const pending = result.pendingUpdate;
        if (pending === undefined && result.digest === installation.digest) {
          toast.info(`${manifest.name} is up to date`);
          return;
        }
        toast.info(pending ? `${manifest.name} ${pending.manifest.version} is ready to review` : `${manifest.name} changed`, {
          description: pending ? pluginCurrentRevisionNote(result) : "Review the new files before they can run.",
          action: { label: "Review", onClick: onReview },
        });
      }),
    repair: () => withBusy(`Couldn't prepare ${manifest.name} for repair`, () => openRepairDraft(installation)),
    stageRepair: () =>
      withBusy(`Couldn't stage the ${manifest.name} fix`, async () => {
        const workspace = installation.recovery?.repair;
        if (!workspace) return;
        await platformStageRepair({ installationId: id, workspaceId: workspace.workspaceId, baseDigest: workspace.baseDigest });
        onReview();
      }),
  };
}

/** The selected plugin: what needs doing first, then its settings, then everything else. */
export function PlatformPluginEditor({
  installation,
  status,
  executionDisabled,
  recoverySupported,
  startViewId,
  headingRef,
  showSwitch,
  toggleBusy,
  onToggle,
  onStartPage,
  onDialog,
}: {
  installation: PluginInstallation;
  status: PluginStatusPresentation;
  executionDisabled: boolean;
  recoverySupported: boolean;
  /** This installation's page this device starts on, if any. */
  startViewId: string | null;
  headingRef: Ref<HTMLHeadingElement>;
  /** On a narrow screen the list and its switches are hidden, so the editor carries one. */
  showSwitch: boolean;
  toggleBusy: boolean;
  onToggle: (installation: PluginInstallation, enabled: boolean) => void;
  onStartPage: (next: PluginStartPage | null) => void;
  onDialog: (dialog: PluginDialog) => void;
}) {
  const { manifest, installationId: id } = installation;
  const readOnly = isPluginRecordReadOnly(installation);
  const unavailable = isPluginUnavailable(installation);
  const version = pluginDisplayVersion(installation);
  const recovery = pluginRecoveryActions(installation, recoverySupported);
  const review = () => onDialog({ kind: "review", installationId: id });
  const lifecycle = usePluginLifecycle(installation, recoverySupported, review);
  const working = toggleBusy || lifecycle.busy;

  return (
    <div data-platform-plugin-editor={id}>
      <div className="px-5 pt-4 pb-3">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <h4
                ref={headingRef}
                tabIndex={-1}
                className="min-w-0 text-body-lg font-semibold text-foreground outline-none [overflow-wrap:anywhere]"
              >
                {manifest.name}
              </h4>
              {version ? <span className="font-mono text-label break-all text-muted-foreground/70">v{version}</span> : null}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-1.5">
              <Badge variant="outline" className={TONE_BADGE[status.tone]} data-plugin-status-label>
                <span aria-hidden className={cn("size-1.5 rounded-full", TONE_DOT[status.tone])} />
                {status.label}
              </Badge>
              {recovery.runningLocalFix ? <Badge variant="outline">Local fix</Badge> : null}
            </div>
          </div>
          {showSwitch ? (
            <Switch
              className={cn(COMPACT_SWITCH, "mt-1")}
              checked={installation.enabled}
              disabled={working || readOnly || (!installation.enabled && unavailable)}
              aria-label={`${installation.enabled ? "Turn off" : "Turn on"} ${manifest.name}`}
              onCheckedChange={(enabled) => onToggle(installation, enabled)}
            />
          ) : null}
        </div>
        {manifest.description ? (
          <p className="mt-2 line-clamp-3 text-body-sm text-muted-foreground [overflow-wrap:anywhere]">{manifest.description}</p>
        ) : null}
      </div>

      <PluginNotices
        installation={installation}
        status={status}
        executionDisabled={executionDisabled}
        canRepair={recovery.repair !== null}
        disabled={working}
        onReview={review}
        onResume={() => void lifecycle.resume()}
        onRepair={() => void lifecycle.repair()}
        onDetails={() => onDialog({ kind: "details", installationId: id })}
      />

      {readOnly ? null : (
        <>
          {manifest.settings.length > 0 ? (
            // Other files may declare other settings, so field errors never carry across a digest.
            <PlatformPluginSettingsForm key={installation.digest} installation={installation} />
          ) : null}
          <PluginViews
            installation={installation}
            executionDisabled={executionDisabled}
            startViewId={startViewId}
            onStartPage={onStartPage}
          />
          <PluginActions installation={installation} executionDisabled={executionDisabled} />
          <PluginPermissions installation={installation} disabled={working} onReview={review} />
          <DetailSection title="Source">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <div className="min-w-0 flex-[1_1_14rem]">
                <p className="text-body-sm text-foreground">
                  {installation.source.kind === "local" ? "Folder on this computer" : "Git repository"}
                </p>
                <p className="font-mono text-label break-all text-muted-foreground">{pluginSourceLabel(installation.source)}</p>
              </div>
              <Button size="sm" variant="outline" disabled={working} onClick={() => void lifecycle.refresh()}>
                <RefreshCw />
                {recovery.runningLocalFix ? "Check publisher for updates" : "Check for changes"}
              </Button>
            </div>
          </DetailSection>
          <DetailSection title="Manage">
            <div className="grid gap-3">
              {recovery.repair !== null ? (
                <ManageRow
                  title="Repair with an agent"
                  description="Copies the running files into a folder an agent can fix. Nothing changes until you review the fix."
                >
                  <Button size="sm" variant="outline" disabled={working} onClick={() => void lifecycle.repair()}>
                    <Bot />
                    {recovery.repair === "open" ? "Open repair draft" : "Repair with agent"}
                  </Button>
                </ManageRow>
              ) : null}
              {recovery.stageRepair ? (
                <ManageRow title="Local fix" description="Review the repair folder's current files before they run.">
                  <Button size="sm" variant="outline" disabled={working} onClick={() => void lifecycle.stageRepair()}>
                    <FileSearch />
                    Review local fix
                  </Button>
                </ManageRow>
              ) : null}
              {recovery.usePrevious !== null ? (
                <ManageRow
                  title="Previous version"
                  description={`v${recovery.usePrevious.version} is kept. It would run with today's settings and data.`}
                >
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={working}
                    onClick={() => onDialog({ kind: "use-previous", installationId: id })}
                  >
                    <History />
                    Use v{recovery.usePrevious.version}…
                  </Button>
                </ManageRow>
              ) : null}
              <ManageRow title="Remove" description="You choose whether to keep its settings and data.">
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-destructive"
                  disabled={working}
                  onClick={() => onDialog({ kind: "remove", installationId: id })}
                >
                  <Trash2 />
                  Remove…
                </Button>
              </ManageRow>
            </div>
          </DetailSection>
        </>
      )}

      <FoldedSection title="Details">
        <div className="grid gap-3">
          {readOnly ? null : (
            <dl className="grid gap-2 text-body-sm">
              {[
                { label: "Identifier", value: manifest.id },
                { label: "Fingerprint", value: installation.digest },
                { label: "Installation", value: id },
              ].map((row) => (
                <div key={row.label} className="min-w-0">
                  <dt className="text-label text-muted-foreground">{row.label}</dt>
                  <dd className="font-mono text-label break-all text-foreground">{row.value}</dd>
                </div>
              ))}
            </dl>
          )}
          <ManageRow title="History and diagnostics" description="What Codemux recorded about its versions and failures.">
            <Button size="sm" variant="outline" onClick={() => onDialog({ kind: "details", installationId: id })}>
              <Info />
              View details
            </Button>
          </ManageRow>
        </div>
      </FoldedSection>
      {lifecycle.backlogDialog}
    </div>
  );
}

/** A labelled control inside a section; the control wraps under the words when space runs out. */
function ManageRow({ title, description, children }: { title: string; description?: string; children?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <div className="min-w-0 flex-[1_1_14rem]">
        <p className="text-body-sm text-foreground">{title}</p>
        {description ? <p className="text-label text-muted-foreground">{description}</p> : null}
      </div>
      {children}
    </div>
  );
}

/** A section that starts closed; its heading is the toggle. */
function FoldedSection({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  return (
    <section aria-labelledby={`${id}-toggle`} className="border-t border-hairline px-5 py-2">
      <button
        id={`${id}-toggle`}
        type="button"
        aria-expanded={open}
        aria-controls={`${id}-body`}
        className="-mx-1.5 flex w-[calc(100%+0.75rem)] items-center gap-1.5 rounded-md px-1.5 py-2 text-left text-label font-medium text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
        onClick={() => setOpen((current) => !current)}
      >
        <ChevronRight
          aria-hidden
          className={cn("size-3.5 shrink-0 transition-transform duration-150 motion-reduce:transition-none", open && "rotate-90")}
        />
        {title}
      </button>
      <Collapse open={open} id={`${id}-body`}>
        <div className="pt-1 pb-2">{children}</div>
      </Collapse>
    </section>
  );
}

function Notice({
  tone,
  title,
  children,
  actions,
}: {
  tone: "info" | "warning" | "error";
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  const Icon = tone === "info" ? Info : tone === "warning" ? ShieldCheck : TriangleAlert;
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cn(
        "flex items-start gap-2.5 rounded-md border px-3 py-2.5 text-body-sm",
        tone === "error"
          ? "border-destructive/40"
          : tone === "warning"
            ? "border-status-attention/40"
            : "border-hairline",
      )}
    >
      <Icon
        aria-hidden
        className={cn(
          "mt-0.5 size-3.5 shrink-0",
          tone === "error" ? "text-destructive" : tone === "warning" ? "text-status-attention" : "text-muted-foreground",
        )}
      />
      <div className="min-w-0 flex-1">
        <p className={cn("font-medium", tone === "error" ? "text-destructive" : "text-foreground")}>{title}</p>
        {children ? <div className="mt-0.5 text-muted-foreground [overflow-wrap:anywhere]">{children}</div> : null}
        {actions ? <div className="mt-2 flex flex-wrap gap-2">{actions}</div> : null}
      </div>
    </div>
  );
}

/** The next thing to do, with the problem in full so it is never only in a menu. */
function PluginNotices({
  installation,
  status,
  executionDisabled,
  canRepair,
  disabled,
  onReview,
  onResume,
  onRepair,
  onDetails,
}: {
  installation: PluginInstallation;
  status: PluginStatusPresentation;
  executionDisabled: boolean;
  canRepair: boolean;
  disabled: boolean;
  onReview: () => void;
  onResume: () => void;
  onRepair: () => void;
  onDetails: () => void;
}) {
  const attention = pluginAttention(installation);
  const button = (label: string, icon: ReactNode, onClick: () => void, busy = disabled) => (
    <Button size="xs" variant="outline" disabled={busy} onClick={onClick}>
      {icon}
      {label}
    </Button>
  );
  // Reading what happened never waits on an action.
  const details = button("View details", <Info />, onDetails, false);
  const notices: ReactNode[] = [];

  if (isPluginRecordReadOnly(installation)) {
    notices.push(
      <Notice key="read-only" tone="info" title="Read-only record" actions={details}>
        {pluginRecordIssueMessage(installation)}
      </Notice>,
    );
  } else {
    if (status.primary === "review") {
      notices.push(
        <Notice key="review" tone="warning" title={status.label} actions={button("Review", <ShieldCheck />, onReview)}>
          {installation.consentedDigest === null
            ? "Nothing from this plugin runs until you choose what it may do."
            : "Its files changed since you approved them. It stays stopped until you approve this version."}
        </Notice>,
      );
    } else if (installation.pendingUpdate) {
      const revision = pluginReviewSubject(installation).revision;
      const version = installation.pendingUpdate.manifest.version;
      notices.push(
        <Notice
          key="staged"
          tone="info"
          title={
            revision === "local-repair"
              ? "Local fix ready to review"
              : revision === "previous"
                ? `Version ${version} ready to review`
                : `Update ${version} ready to review`
          }
          actions={button(REVIEW_LABELS[revision], <ShieldCheck />, onReview)}
        >
          {pluginCurrentRevisionNote(installation)}
        </Notice>,
      );
    }
    // An off plugin that can't run here still says why, as information rather than a problem.
    if (attention !== null && attention.kind !== "read-only") {
      const primary =
        status.primary === "resume" && !executionDisabled
          ? button("Restart", <RotateCcw />, onResume)
          : attention.kind === "events-paused" && installation.enabled && !executionDisabled
            ? button("Resume events", <RotateCcw />, onResume)
            : isPluginUnavailable(installation) && canRepair
              ? button("Repair with agent", <Bot />, onRepair)
              : null;
      notices.push(
        <Notice
          key="attention"
          tone={!attention.notify ? "info" : attention.kind === "events-paused" ? "warning" : "error"}
          title={attention.title}
          actions={
            <>
              {primary}
              {details}
            </>
          }
        >
          {attention.message}
        </Notice>,
      );
    }
  }
  if (notices.length === 0) return null;
  return <div className="grid gap-2 px-5 pb-4">{notices}</div>;
}

function PluginViews({
  installation,
  executionDisabled,
  startViewId,
  onStartPage,
}: {
  installation: PluginInstallation;
  executionDisabled: boolean;
  startViewId: string | null;
  onStartPage: (next: PluginStartPage | null) => void;
}) {
  const { manifest } = installation;
  const layouts = [...(manifest.sidebarViews ?? []), ...(manifest.nativeTrees ?? [])];
  if (manifest.views.length === 0 && layouts.length === 0) return null;
  const blocked = executionDisabled
    ? "Plugin code is turned off for this launch, so its views can't open."
    : !isPluginActive(installation)
      ? (pluginInactiveReason(installation) ?? "The plugin is not running.")
      : !installation.grants.capabilities.includes("ui")
        ? "Showing views isn't allowed. Change this in Permissions."
        : null;
  // From Settings there is no chat beside it, so a side panel opens as a page.
  const open = (viewId: string) =>
    applyPluginIntents(capturePluginTarget({ installation, workspaceId: null, threadId: null, composer: false }), [
      { type: "view.open", viewId },
    ]);
  return (
    <DetailSection title="Views">
      <div className="grid gap-3">
        {manifest.views.length > 0 && blocked ? <p className="text-label text-muted-foreground">{blocked}</p> : null}
        {manifest.views.map((view) => {
          const chosen = startViewId === view.id;
          return (
            <ManageRow
              key={`view:${view.id}`}
              title={view.title}
              description={
                view.placement === "page"
                  ? "Page"
                  : view.placement === "conversation"
                    ? "Card in chat. It appears when the plugin adds it to a conversation."
                    : "Side panel beside a chat. From here it opens as a page."
              }
            >
              {view.placement === "conversation" ? null : (
                <div className="flex flex-wrap items-center gap-1">
                  {view.placement === "page" ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-pressed={chosen}
                      title={chosen ? "This device opens this page when Codemux starts" : "Open this page when Codemux starts on this device"}
                      aria-label={chosen ? `Stop starting on ${view.title}` : `Start on ${view.title} on this device`}
                      onClick={() => onStartPage(chosen ? null : pluginStartPageFor(installation, view.id))}
                    >
                      <House />
                      {chosen ? "Start page" : "Start here"}
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={blocked !== null}
                    aria-label={`Open ${view.title}`}
                    onClick={() => open(view.id)}
                  >
                    <SquareArrowOutUpRight />
                    Open
                  </Button>
                </div>
              )}
            </ManageRow>
          );
        })}
        {layouts.map((layout) => (
          <ManageRow
            key={`layout:${layout.id}`}
            title={layout.title}
            description="Sidebar layout. Off until you pick it from the sidebar's layout menu."
          />
        ))}
      </div>
    </DetailSection>
  );
}

function PluginActions({
  installation,
  executionDisabled,
}: {
  installation: PluginInstallation;
  executionDisabled: boolean;
}) {
  const { actions } = installation.manifest;
  const [running, setRunning] = useState<string | null>(null);
  if (actions.length === 0) return null;
  const active = isPluginActive(installation) && !executionDisabled;
  return (
    <FoldedSection title={`Actions (${actions.length})`}>
      <div className="grid gap-3">
        {actions.map((action) => (
          <ManageRow
            key={action.id}
            title={action.title}
            description={[
              action.description,
              action.placements.map((placement) => PLACEMENT_LABELS[placement] ?? placement).join(", "),
              action.agentCallable ? "Agents can call it" : undefined,
            ]
              .filter(Boolean)
              .join(" · ")}
          >
            <Button
              size="sm"
              variant="ghost"
              disabled={!active || running !== null}
              aria-label={`Run ${action.title}`}
              onClick={async () => {
                setRunning(action.id);
                try {
                  await runPluginAction(
                    capturePluginTarget({ installation, workspaceId: null, threadId: null, composer: false }),
                    action,
                    {},
                  );
                } finally {
                  setRunning(null);
                }
              }}
            >
              <Play />
              Run
            </Button>
          </ManageRow>
        ))}
      </div>
    </FoldedSection>
  );
}

function PluginPermissions({
  installation,
  disabled,
  onReview,
}: {
  installation: PluginInstallation;
  disabled: boolean;
  onReview: () => void;
}) {
  const { manifest, grants } = installation;
  const known = usePluginKnownProjects();
  const granted = describePluginCapabilities(manifest).filter(
    (entry) => entry.known && grants.capabilities.includes(entry.capability),
  );
  const titles = new Map(known.map((project) => [project.id, project.title]));
  const projects = grants.projectIds.map((projectId) => titles.get(projectId) ?? "Unknown project");
  return (
    <DetailSection
      title="Permissions"
      action={
        <Button size="xs" variant="outline" disabled={disabled} onClick={onReview}>
          <ShieldCheck />
          Review permissions
        </Button>
      }
    >
      <p className="text-body-sm text-foreground">
        {granted.length === 0 ? "No permissions granted." : granted.map((entry) => entry.title).join(" · ")}
      </p>
      {pluginNeedsProjects(manifest) ? (
        <p className="mt-1 text-label text-muted-foreground [overflow-wrap:anywhere]">
          {projects.length === 0
            ? "No projects chosen. Chat and background permissions apply only to projects you pick."
            : `Projects: ${projects.join(", ")}`}
        </p>
      ) : null}
    </DetailSection>
  );
}
