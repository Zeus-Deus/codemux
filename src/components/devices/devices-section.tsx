import { useCallback, useState, type ReactNode } from "react";

import {
  ArrowDownToLine,
  ChevronRight,
  Folder,
  Loader2,
  Plus,
  Trash2,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { formatBytes } from "@/lib/format-bytes";
import { useElapsedSeconds } from "@/hooks/use-elapsed-seconds";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import {
  workspacesAdoptProject,
  workspacesSyncNow,
  type WorkspaceSyncView,
} from "@/tauri/commands";

import { PullToDeviceDialog } from "./pull-to-device-dialog";
import { SweepDialog } from "./sweep-dialog";
import { runOpenOnHost } from "./open-on-host-action";
import {
  cardRowCount,
  useDeviceCards,
  type DeviceCard,
  type DeviceProject,
  type DeviceRow,
  type DeviceTone,
} from "./use-device-cards";
import { useSweepCandidates } from "./use-sweep-candidates";

/** The hosted web client; the user picks the device there. */
const HOSTED_CLIENT_URL = "https://app.codemux.org";

/**
 * Body of the Devices page. One scroll column: the local one-liner with its
 * sweep chip, a card per configured device (its synced workspaces grouped by
 * project), then "Add device". Local workspaces are deliberately absent —
 * the sidebar owns them; this page moves work between machines.
 */
export function DevicesSection() {
  const cards = useDeviceCards();
  const loaded = useAppStore((s) => s.appState !== null);
  const localWorkspaceCount = useAppStore(
    (s) => s.appState?.workspaces.length ?? 0,
  );
  const setShowDevices = useUIStore((s) => s.setShowDevices);
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const setTransferError = useAppStore((s) => s.setWorkspacePushPullError);

  // Pull dialog: the sync row whose adoption is in progress; null = closed.
  const [pullRow, setPullRow] = useState<WorkspaceSyncView | null>(null);

  // Project-first pull: root + every worktree in one action.
  const [pullingProjectUid, setPullingProjectUid] = useState<string | null>(null);
  const handlePullProject = useCallback(
    async (projectUid: string, projectName: string) => {
      setPullingProjectUid(projectUid);
      try {
        const result = await workspacesAdoptProject(projectUid);
        if (result.failures.length > 0) {
          toast.error(
            `Pulled ${projectName} with ${result.failures.length} issue(s)`,
            {
              description: result.failures
                .map((f) => `${f.title}: ${f.error}`)
                .join("\n"),
            },
          );
          setTransferError(`Pull failed: ${projectName}`);
        } else {
          toast.success(`Pulled ${projectName} to this device`, {
            description: result.message,
          });
          setTransferError(null);
        }
        await workspacesSyncNow().catch(() => {});
      } catch (err) {
        toast.error(`Failed to pull ${projectName}`, {
          description: err instanceof Error ? err.message : String(err),
        });
        // Keeps the footer's device dot amber until a later transfer runs.
        setTransferError(`Pull failed: ${projectName}`);
      } finally {
        setPullingProjectUid(null);
      }
    },
    [setTransferError],
  );

  const closeDevices = useCallback(() => setShowDevices(false), [setShowDevices]);
  const openHostSettings = useCallback(() => {
    setShowDevices(false);
    setShowSettings(true, "hosts");
  }, [setShowDevices, setShowSettings]);

  if (!loaded) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" />
        Loading devices…
      </div>
    );
  }

  return (
    <div className="h-full min-h-0 overflow-y-auto px-4 pt-3 pb-5">
      <div className="mx-auto flex max-w-[720px] flex-col gap-2.5">
        <ThisDeviceRow workspaceCount={localWorkspaceCount} />

        {cards.length === 0 ? (
          <EmptyDevices />
        ) : (
          cards.map((card) => (
            <DeviceCardView
              key={card.key}
              card={card}
              pullingProjectUid={pullingProjectUid}
              onPullProject={handlePullProject}
              onRequestPull={setPullRow}
              onOpened={closeDevices}
            />
          ))
        )}

        <AddDeviceRow onClick={openHostSettings} />
      </div>

      <PullToDeviceDialog
        syncRow={pullRow}
        onOpenChange={(open) => {
          if (!open) setPullRow(null);
        }}
      />
    </div>
  );
}

// ── This device ─────────────────────────────────────────────────

function ThisDeviceRow({ workspaceCount }: { workspaceCount: number }) {
  const [sweepOpen, setSweepOpen] = useState(false);
  // The chip appears with the count as soon as the backend says which
  // settled worktrees qualify; the "~1.2 GB" fragment sums whatever sizes
  // it could measure.
  const { candidates, knownBytes } = useSweepCandidates();
  const count = candidates.length;

  return (
    <div className="flex items-center gap-2.5 rounded-[10px] bg-foreground/[0.035] px-3 py-2.5">
      <LaptopGlyph className="size-[13px] shrink-0 text-status-open" />
      <span className="text-[12px] font-semibold text-foreground">
        This device
      </span>
      <span className="truncate font-mono text-[10px] text-muted-foreground/70">
        {workspaceCount} {workspaceCount === 1 ? "workspace" : "workspaces"} ·
        managed in the sidebar
      </span>
      {count > 0 && (
        <button
          type="button"
          onClick={() => setSweepOpen(true)}
          className="ml-auto inline-flex shrink-0 items-center gap-1.5 rounded-[7px] bg-status-working/[0.11] px-2.5 py-[5px] text-[11px] font-semibold text-status-working transition-colors hover:bg-status-working/[0.17]"
        >
          <Trash2 className="size-2.5" aria-hidden />
          Sweep {count} settled
          {knownBytes !== null && knownBytes > 0 && (
            <span className="font-medium text-status-working/80">
              · ~{formatBytes(knownBytes)}
            </span>
          )}
        </button>
      )}
      <SweepDialog
        open={sweepOpen}
        onOpenChange={setSweepOpen}
        candidates={candidates}
        knownBytes={knownBytes}
      />
    </div>
  );
}

// ── Device card ─────────────────────────────────────────────────

const TONE_DOT: Record<DeviceTone, string> = {
  online: "bg-status-open ring-[3px] ring-status-open/15",
  attention: "bg-status-working ring-[3px] ring-status-working/15",
  offline: "bg-muted-foreground/60",
  checking: "bg-muted-foreground/40",
};

const TONE_TEXT: Record<DeviceTone, string> = {
  online: "text-status-open",
  attention: "text-status-working",
  offline: "text-muted-foreground/70",
  checking: "text-muted-foreground/60",
};

function DeviceCardView({
  card,
  pullingProjectUid,
  onPullProject,
  onRequestPull,
  onOpened,
}: {
  card: DeviceCard;
  pullingProjectUid: string | null;
  onPullProject: (projectUid: string, projectName: string) => void;
  onRequestPull: (row: WorkspaceSyncView) => void;
  onOpened: () => void;
}) {
  // A reachable device is one you can act on, so its card starts open;
  // everything else folds to its header. A click overrides either default
  // until the page remounts.
  const reachable = card.tone === "online" || card.tone === "attention";
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const expanded = expandedOverride ?? reachable;
  const toggle = () => setExpandedOverride(!expanded);

  const rowCount = cardRowCount(card);
  const projectCount = card.projects.length;
  const meta = [
    card.host?.ssh_target ?? null,
    card.host && !card.serverId
      ? "not synced yet"
      : `${rowCount} ${rowCount === 1 ? "workspace" : "workspaces"}`,
    card.serverId
      ? `${projectCount} ${projectCount === 1 ? "project" : "projects"}`
      : null,
    card.diskBytes !== null ? formatBytes(card.diskBytes) : null,
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <section
      aria-label={card.name}
      className={cn(
        "overflow-hidden rounded-xl",
        reachable
          ? "bg-card shadow-[0_3px_14px_rgba(0,0,0,0.28)]"
          : "bg-foreground/[0.03] opacity-80",
      )}
    >
      <div className="flex items-center gap-2.5 px-[13px] py-3">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={expanded}
          aria-label={`${expanded ? "Collapse" : "Expand"} ${card.name}`}
          className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
        >
          <span
            className={cn(
              "flex size-7 shrink-0 items-center justify-center rounded-[9px]",
              reachable
                ? "bg-status-remote/[0.13] text-status-remote"
                : "bg-foreground/[0.06] text-muted-foreground/70",
            )}
          >
            <DeviceGlyph className="size-[13px]" />
          </span>
          <span className="flex min-w-0 flex-col gap-px">
            <span className="flex items-center gap-[7px]">
              <span className="truncate text-[13px] font-bold text-foreground">
                {card.name}
              </span>
              <span
                aria-hidden
                className={cn("size-1.5 shrink-0 rounded-full", TONE_DOT[card.tone])}
              />
              <span
                title={card.statusDetail ?? undefined}
                className={cn("truncate text-[10.5px]", TONE_TEXT[card.tone])}
              >
                {card.statusLabel}
              </span>
            </span>
            {/* A degraded device says what is wrong in place of its meta
                line — that is the only thing worth reading about it. */}
            <span
              className={cn(
                "truncate font-mono text-[9.5px]",
                card.tone === "attention"
                  ? "text-status-working/80"
                  : "text-muted-foreground/70",
              )}
            >
              {card.tone === "attention" && card.statusDetail
                ? card.statusDetail
                : meta}
            </span>
          </span>
        </button>

        {card.remoteControlServing && (
          <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-status-open/10 px-2.5 py-1 text-[10.5px] font-semibold text-status-open">
            <span aria-hidden className="size-[5px] rounded-full bg-status-open" />
            Remote Control serving
          </span>
        )}
        {card.remoteControlServing ? (
          <button
            type="button"
            onClick={() => void openUrl(HOSTED_CLIENT_URL)}
            className="shrink-0 rounded-[7px] bg-accent-ember px-3 py-1.5 text-[11px] font-semibold text-white shadow-[0_2px_8px_rgba(0,0,0,0.25)] transition-[filter] hover:brightness-110"
          >
            Connect
          </button>
        ) : (
          <ChevronRight
            aria-hidden
            className={cn(
              "size-[11px] shrink-0 text-muted-foreground/70 transition-transform",
              expanded && "rotate-90",
            )}
          />
        )}
      </div>

      {expanded && rowCount > 0 && (
        <div className="flex flex-col gap-2 px-2.5 pb-2.5 pt-0.5">
          {card.projects.map((project, index) => (
            <ProjectCluster
              key={project.key}
              card={card}
              project={project}
              initiallyExpanded={index === 0}
              pulling={
                project.projectUid !== null &&
                pullingProjectUid === project.projectUid
              }
              onPullProject={onPullProject}
              onRequestPull={onRequestPull}
              onOpened={onOpened}
            />
          ))}
        </div>
      )}
      {expanded && rowCount === 0 && card.host && card.serverId && (
        <p className="px-[13px] pb-3 text-[11px] text-muted-foreground/70">
          Nothing on {card.name} yet — push a workspace here from its menu in
          the sidebar.
        </p>
      )}
    </section>
  );
}

// ── Project cluster ─────────────────────────────────────────────

function ProjectCluster({
  card,
  project,
  initiallyExpanded,
  pulling,
  onPullProject,
  onRequestPull,
  onOpened,
}: {
  card: DeviceCard;
  project: DeviceProject;
  initiallyExpanded: boolean;
  pulling: boolean;
  onPullProject: (projectUid: string, projectName: string) => void;
  onRequestPull: (row: WorkspaceSyncView) => void;
  onOpened: () => void;
}) {
  // The first project opens; the rest read as one-line headers until clicked.
  const [expanded, setExpanded] = useState(initiallyExpanded);

  return (
    <div>
      <div
        className={cn(
          "flex items-center gap-1.5 rounded-lg px-1 py-1",
          !expanded && "hover:bg-foreground/[0.04]",
        )}
      >
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
        >
          <Folder className="size-[11px] shrink-0 text-muted-foreground/60" aria-hidden />
          <span className="truncate font-mono text-[11px] font-semibold text-foreground/80">
            {project.name}
          </span>
          <span className="font-mono text-[9.5px] text-muted-foreground/60">
            {project.rows.length}
          </span>
          {!expanded && (
            <ChevronRight
              aria-hidden
              className="ml-0.5 size-[11px] text-muted-foreground/60"
            />
          )}
        </button>
        {expanded && project.projectUid && (
          <button
            type="button"
            disabled={pulling}
            onClick={() => onPullProject(project.projectUid!, project.name)}
            title="Pull this project's repo root and all its worktrees to this device"
            className="ml-auto inline-flex shrink-0 items-center gap-1 rounded-md px-2 py-[3px] text-[10px] font-semibold text-muted-foreground transition-colors hover:bg-foreground/[0.07] hover:text-foreground disabled:opacity-60"
          >
            {pulling ? (
              <Loader2 className="size-[9px] animate-spin" aria-hidden />
            ) : (
              <ArrowDownToLine className="size-[9px]" aria-hidden />
            )}
            Pull project
          </button>
        )}
      </div>
      {expanded && (
        <ul className="flex flex-col gap-0.5">
          {project.rows.map((row) => (
            <WorkspaceRow
              key={row.sync.id}
              row={row}
              card={card}
              onRequestPull={onRequestPull}
              onOpened={onOpened}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

// ── Workspace row ───────────────────────────────────────────────

function WorkspaceRow({
  row,
  card,
  onRequestPull,
  onOpened,
}: {
  row: DeviceRow;
  card: DeviceCard;
  onRequestPull: (row: WorkspaceSyncView) => void;
  onOpened: () => void;
}) {
  const { sync } = row;
  const [opening, setOpening] = useState(false);
  // Opening in place needs an SSH route, so only configured hosts offer it.
  const canOpenOnHost = card.host !== null;

  // A pull keyed on this row's server id is in flight. The row watches its
  // own key so an unrelated transfer never re-renders the whole page.
  const pullStartedAt = useAppStore((s) =>
    s.workspacePushPullInFlight === `pending-adopt-${sync.server_id}`
      ? s.workspacePushPullStartedAt
      : null,
  );
  const elapsedSec = useElapsedSeconds(pullStartedAt);

  const handleOpen = async () => {
    if (opening) return;
    setOpening(true);
    try {
      await runOpenOnHost(sync, card.name, onOpened);
    } finally {
      setOpening(false);
    }
  };

  const kindLabel =
    sync.workspace_kind === "main"
      ? "repo root"
      : sync.workspace_kind === "worktree"
        ? "worktree"
        : null;

  return (
    <li className="grid h-8 grid-cols-[14px_minmax(0,1fr)_auto_auto] items-center gap-2 rounded-lg px-2 hover:bg-foreground/[0.05]">
      <span
        aria-hidden
        className="size-1.5 justify-self-center rounded-full bg-status-remote/70"
      />
      <span className="flex min-w-0 items-center">
        <span className="truncate text-[12px] font-semibold text-foreground">
          {sync.title}
        </span>
        {kindLabel && (
          <span className="ml-1 shrink-0 rounded-full bg-foreground/[0.06] px-[7px] py-[2px] font-mono text-[9px] font-medium text-muted-foreground/70">
            {kindLabel}
          </span>
        )}
        {row.divergedLabel && (
          <span
            title={`Same branch has different commits on ${row.divergedLabel}`}
            className="ml-1 shrink-0 rounded-full bg-status-working/[0.11] px-[7px] py-[2px] font-mono text-[9px] font-semibold text-status-working"
          >
            diverged
          </span>
        )}
      </span>
      <span className="truncate font-mono text-[9.5px] text-muted-foreground/70">
        {sync.git_branch ?? ""}
      </span>
      <span className="inline-flex items-center gap-1">
        {pullStartedAt !== null ? (
          <span className="inline-flex items-center gap-1.5 px-2 text-[10px] font-semibold text-muted-foreground">
            <Loader2 className="size-3 animate-spin" aria-hidden />
            Pulling
            {elapsedSec !== null && (
              <span className="font-mono tabular-nums">{elapsedSec}s</span>
            )}
          </span>
        ) : (
          <>
            {canOpenOnHost && (
              <RowAction onClick={() => void handleOpen()} disabled={opening}>
                {opening ? (
                  <Loader2 className="size-3 animate-spin" aria-hidden />
                ) : (
                  `Open on ${card.name}`
                )}
              </RowAction>
            )}
            <RowAction onClick={() => onRequestPull(sync)}>Pull here</RowAction>
          </>
        )}
      </span>
    </li>
  );
}

function RowAction({
  onClick,
  disabled,
  children,
}: {
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="rounded-md bg-foreground/[0.07] px-[9px] py-1 text-[10px] font-semibold text-muted-foreground transition-colors hover:bg-foreground/[0.11] hover:text-foreground disabled:opacity-60"
    >
      {children}
    </button>
  );
}

// ── Add device / empty state ────────────────────────────────────

function AddDeviceRow({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex items-center justify-center gap-1.5 rounded-[10px] bg-foreground/[0.03] p-2.5 text-[11.5px] font-semibold text-muted-foreground/70 transition-colors hover:bg-foreground/[0.05] hover:text-muted-foreground"
    >
      <Plus className="size-2.5" aria-hidden strokeWidth={2.2} />
      Add device
    </button>
  );
}

function EmptyDevices() {
  return (
    <div className="px-6 py-8 text-center">
      <p className="text-[13px] font-semibold text-foreground">No devices yet</p>
      <p className="mx-auto mt-1.5 max-w-[420px] text-[12px] leading-relaxed text-muted-foreground/80">
        Add a device — your home desktop, an always-on box, or a cloud server
        — then push work to it from any workspace's menu.
      </p>
    </div>
  );
}

// ── Glyphs ──────────────────────────────────────────────────────

function LaptopGlyph({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      className={className}
    >
      <rect x="2.5" y="3" width="11" height="7" rx="1.2" />
      <path d="M1 12.5h14" />
    </svg>
  );
}

function DeviceGlyph({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      className={className}
    >
      <rect x="3" y="1.8" width="10" height="12.4" rx="1.4" />
      <circle cx="8" cy="11.6" r="0.9" fill="currentColor" stroke="none" />
      <path d="M5.5 4.4h5M5.5 6.6h5" />
    </svg>
  );
}
