import { useCallback, useEffect, useState } from "react";

import {
  AlertTriangle,
  ArrowDownToLine,
  ChevronDown,
  ChevronRight,
  Cloud,
  GitBranch,
  Loader2,
  Settings2,
} from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useWorkspacesSyncStore } from "@/stores/workspaces-sync-store";
import {
  activateWorkspace,
  workspacePushToHost,
  workspacesAdoptionPreview,
  workspacesAdoptSynced,
  workspacesAdoptViaClone,
  type AdoptionPreview,
  type WorkspaceSyncView,
} from "@/tauri/commands";
import { useHostsStore } from "@/stores/hosts-store";

import { remoteProjectName } from "./use-overview-items";

interface Props {
  /** The synced row the user wants to adopt. Null = dialog closed. */
  syncRow: WorkspaceSyncView | null;
  onOpenChange: (open: boolean) => void;
}

const FIRST_PULL_DONE_KEY_PREFIX = "codemux.workspaces.firstPullDone.";

function firstPullDoneKey(): string {
  // Per-device machine id would be ideal; for v1 use a single global
  // flag scoped by the app's persistence root. The cost of being
  // slightly imprecise here is that a user on a multi-account
  // machine sees the disclosure twice — acceptable.
  return `${FIRST_PULL_DONE_KEY_PREFIX}default`;
}

function hasSeenFirstPull(): boolean {
  try {
    return localStorage.getItem(firstPullDoneKey()) === "1";
  } catch {
    return false;
  }
}

function markFirstPullSeen() {
  try {
    localStorage.setItem(firstPullDoneKey(), "1");
  } catch {
    // localStorage disabled — non-fatal, just always pre-expand.
  }
}

/**
 * Pull-to-this-device dialog. The frontend opens this when the user
 * clicks "Pull to this device" on a sibling-device row in the
 * Workspaces overview.
 *
 * Behaviour:
 *   1. On open, fetch the adoption preview via
 *      `workspacesAdoptionPreview`. This tells us whether host-backed
 *      adoption is possible (the headline path), or whether we'd
 *      need the Phase-3 clone fallback (not implemented yet — we
 *      surface the case with a clear message).
 *   2. Render a summary: workspace title, source host, project,
 *      branch, target local path. The "What this does" disclosure
 *      is pre-expanded on the user's first pull on this device.
 *   3. On submit, call `workspacesAdoptSynced` and close the dialog
 *      immediately. The pending state is surfaced via the existing
 *      `workspacePushPullInFlight` machinery — the dialog does NOT
 *      stay open during the rsync.
 *   4. Toast on completion with the option to open the new workspace.
 *
 * Mirrors the existing `CloneDialog` shape (sm:max-w-[460px] popover)
 * and the in-flight signalling pattern used by push-to-host.
 */
export function PullToDeviceDialog({ syncRow, onOpenChange }: Props) {
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );
  const refreshSync = useWorkspacesSyncStore((s) => s.refresh);
  const hosts = useHostsStore((s) => s.hosts);

  const [preview, setPreview] = useState<AdoptionPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [disclosureOpen, setDisclosureOpen] = useState(false);

  const open = syncRow !== null;
  const serverId = syncRow?.server_id ?? null;

  // Load preview when the dialog opens. Aborts on close so we don't
  // leak a pending IPC after the user dismisses.
  useEffect(() => {
    if (!open || !serverId) return;
    let cancelled = false;
    setPreview(null);
    setPreviewError(null);
    setDisclosureOpen(!hasSeenFirstPull());
    workspacesAdoptionPreview(serverId)
      .then((result) => {
        if (!cancelled) setPreview(result);
      })
      .catch((err) => {
        if (!cancelled) {
          setPreviewError(
            err instanceof Error ? err.message : String(err),
          );
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, serverId]);

  // Adoption mode is decided by the preview — host-backed when
  // possible, clone-fallback when the workspace has no shared host
  // but does have a project_remote.
  const isCloneMode =
    !!preview && !preview.can_host_adopt && preview.can_clone_adopt;

  const handleSubmit = useCallback(async () => {
    if (!syncRow || !serverId || !preview) return;
    setSubmitting(true);
    // Optimistic: signal the in-flight state immediately so the
    // sidebar + overview show a spinner the moment the dialog
    // closes. We don't have a local workspace id yet (the shell is
    // created inside the adopt command), so we key the in-flight
    // signal on the server id prefixed to avoid colliding with real
    // workspace ids.
    const inFlightKey = `pending-adopt-${serverId}`;
    setPushPullInFlight(inFlightKey);
    onOpenChange(false);

    try {
      // Two distinct paths with different success-toast shapes:
      // - host-backed: rsync from a shared host; Undo = push back
      //   to that host (data-safety guardrail).
      // - clone: git clone + worktree-add; no Undo (independent
      //   copy is now its own thing).
      if (isCloneMode) {
        const result = await workspacesAdoptViaClone(serverId);
        markFirstPullSeen();
        void refreshSync();
        toast.success(`Cloned ${syncRow.title} to this device`, {
          description:
            "Independent copy created. Commit and push to share changes with your other device.",
          action: {
            label: "Open",
            onClick: () => {
              setShowWorkspacesOverview(false);
              void activateWorkspace(result.workspace_id);
            },
          },
        });
        return;
      }

      // Host-backed branch ↓
      const result = await workspacesAdoptSynced(serverId);
      markFirstPullSeen();
      void refreshSync();

      // Resolve the local hosts.id matching the host we just
      // pulled from — needed for the Undo = push-back flow. The
      // hosts cache is populated by the overview before the dialog
      // ever opens, so this lookup is synchronous.
      const sourceHostServerId = syncRow.host_server_id;
      const sourceHost = sourceHostServerId
        ? hosts.find((h) => h.server_id === sourceHostServerId)
        : null;

      if (sourceHost) {
        // Push-back as Undo: data-safety guardrail. Wrong pull is
        // one click from recovery within 10s.
        toast.undoable({
          message: `Pulled ${syncRow.title} to this device`,
          description: preview.host_label
            ? `From ${preview.host_label}. Tap Undo within 10s to send it back.`
            : result.message,
          onUndo: async () => {
            const undoResult = await workspacePushToHost(
              result.workspace_id,
              sourceHost.id,
            );
            void refreshSync();
            if (undoResult.ok) {
              toast.success(
                `Sent ${syncRow.title} back to ${sourceHost.name}`,
              );
            } else {
              toast.error("Push back failed", {
                description: undoResult.message,
              });
            }
          },
        });
      } else {
        // No source host known on this device (race: it was
        // deleted between pull start and finish). Plain success
        // toast — no Undo target available.
        toast.success(`Pulled ${syncRow.title} to this device`, {
          description: preview.host_label
            ? `From ${preview.host_label}.`
            : result.message,
          action: {
            label: "Open",
            onClick: () => {
              setShowWorkspacesOverview(false);
              void activateWorkspace(result.workspace_id);
            },
          },
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      // Decode our structured-error prefixes so the toast is
      // actionable rather than raw.
      let title = "Pull failed";
      let description = message;
      if (message.startsWith("host_not_configured:")) {
        title = "Configure the device first";
        description = message.replace(/^host_not_configured:\s*/, "");
      } else if (message.startsWith("path_in_use:")) {
        title = "Path already in use";
        description = message.replace(/^path_in_use:\s*/, "");
      }
      toast.error(title, { description });
    } finally {
      setSubmitting(false);
      setPushPullInFlight(null);
    }
  }, [
    syncRow,
    serverId,
    preview,
    isCloneMode,
    setPushPullInFlight,
    onOpenChange,
    refreshSync,
    setShowWorkspacesOverview,
    hosts,
  ]);

  // ── Open-existing-on-already-adopted short-circuit ────────────
  //
  // The user clicked Pull on a row that's already been adopted on
  // this device (rare, e.g. if the overview is stale). Open the
  // existing workspace instead of confusingly trying to "adopt
  // again".
  const alreadyAdopted = preview?.already_adopted_workspace_id ?? null;

  const handleOpenExisting = useCallback(() => {
    if (!alreadyAdopted) return;
    onOpenChange(false);
    setShowWorkspacesOverview(false);
    void activateWorkspace(alreadyAdopted);
  }, [alreadyAdopted, onOpenChange, setShowWorkspacesOverview]);

  // ── Cross-machine same-branch-same-project guard ──────────────
  //
  // The user clicked Pull on a row whose `(project basename,
  // git_branch)` matches a workspace already open on this device.
  // That's the strong "you're already doing this work" signal — show
  // the existing workspace and let them open it, rather than silently
  // creating a parallel copy. Layered AFTER `alreadyAdopted` so an
  // already-adopted row gets the simpler "Open it" copy; this block
  // is for "you have a DIFFERENT local workspace that's logically
  // the same."
  const sameBranchConflict =
    preview?.same_branch_project_exists_at ?? null;

  const handleOpenSameBranchConflict = useCallback(() => {
    if (!sameBranchConflict) return;
    onOpenChange(false);
    setShowWorkspacesOverview(false);
    void activateWorkspace(sameBranchConflict);
  }, [sameBranchConflict, onOpenChange, setShowWorkspacesOverview]);

  if (!syncRow) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="sm:max-w-[480px] bg-popover p-0 gap-0 overflow-hidden"
      >
        <DialogHeader className="px-5 pt-4 pb-2">
          <DialogTitle className="text-[14px] font-semibold">
            Pull workspace to this device
          </DialogTitle>
          <DialogDescription className="text-[12px] text-muted-foreground/80">
            Bring{" "}
            <span className="font-medium text-foreground">
              {syncRow.title}
            </span>{" "}
            from another device.
          </DialogDescription>
        </DialogHeader>

        <div className="px-5 pb-4 space-y-3">
          {previewError ? (
            <ErrorBlock message={previewError} />
          ) : !preview ? (
            <div className="flex items-center gap-2 py-3 text-[12px] text-muted-foreground">
              <Loader2 className="size-3.5 animate-spin" />
              Checking…
            </div>
          ) : alreadyAdopted ? (
            <AlreadyAdoptedBlock onOpen={handleOpenExisting} />
          ) : sameBranchConflict ? (
            <SameBranchProjectBlock
              onOpenExisting={handleOpenSameBranchConflict}
            />
          ) : preview.can_host_adopt && !preview.is_path_in_use ? (
            <HostBackedAdoptionForm
              syncRow={syncRow}
              preview={preview}
              disclosureOpen={disclosureOpen}
              onToggleDisclosure={() =>
                setDisclosureOpen((open) => !open)
              }
            />
          ) : preview.can_host_adopt && preview.is_path_in_use ? (
            <PathInUseBlock path={preview.suggested_path} />
          ) : preview.can_clone_adopt && !preview.is_path_in_use ? (
            <CloneFallbackBlock syncRow={syncRow} preview={preview} />
          ) : preview.can_clone_adopt && preview.is_path_in_use ? (
            <PathInUseBlock path={preview.suggested_path} />
          ) : !preview.host_configured && syncRow.host_server_id ? (
            <HostNotConfiguredBlock
              hostServerId={syncRow.host_server_id}
              onClose={() => onOpenChange(false)}
            />
          ) : (
            <NoOptionsBlock />
          )}

          <div className="flex justify-end gap-2 pt-1 border-t border-border/40">
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-3 text-[12px]"
              onClick={() => onOpenChange(false)}
              disabled={submitting}
            >
              Cancel
            </Button>
            {preview &&
              !alreadyAdopted &&
              !sameBranchConflict &&
              !preview.is_path_in_use &&
              (preview.can_host_adopt || isCloneMode) && (
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 gap-1.5 px-3 text-[12px]"
                  onClick={() => void handleSubmit()}
                  disabled={submitting}
                >
                  {submitting ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : (
                    <ArrowDownToLine className="size-3" />
                  )}
                  {isCloneMode ? "Clone and open" : "Pull workspace"}
                </Button>
              )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

// ── Subviews ────────────────────────────────────────────────────

function HostBackedAdoptionForm({
  syncRow,
  preview,
  disclosureOpen,
  onToggleDisclosure,
}: {
  syncRow: WorkspaceSyncView;
  preview: AdoptionPreview;
  disclosureOpen: boolean;
  onToggleDisclosure: () => void;
}) {
  return (
    <>
      <dl className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[13px] space-y-1.5">
        <SummaryRow
          label="From"
          icon={<Cloud className="size-3 text-status-remote/80" />}
        >
          {preview.host_label ?? "—"}
        </SummaryRow>
        <SummaryRow label="Project">
          {remoteProjectName(syncRow) ?? "—"}
        </SummaryRow>
        <SummaryRow label="Branch">
          <span className="font-mono text-[12px]">
            {syncRow.git_branch ?? "—"}
          </span>
        </SummaryRow>
        <SummaryRow label="Will land">
          <span className="font-mono text-[11px] text-muted-foreground/80 break-all">
            {preview.suggested_path}
          </span>
        </SummaryRow>
      </dl>

      <button
        type="button"
        onClick={onToggleDisclosure}
        className="flex w-full items-center gap-1 text-[12px] text-muted-foreground/75 hover:text-foreground transition-colors"
      >
        {disclosureOpen ? (
          <ChevronDown className="size-3" />
        ) : (
          <ChevronRight className="size-3" />
        )}
        What this does
      </button>

      {disclosureOpen && (
        <ul className="rounded-md bg-muted/20 border border-border/40 px-3 py-2 text-[12px] text-muted-foreground/85 leading-relaxed space-y-1">
          <li>
            • Copies the workspace files from{" "}
            <span className="font-medium text-foreground/90">
              {preview.host_label}
            </span>{" "}
            to this device via rsync.
          </li>
          <li>
            • The copy on {preview.host_label} stays in place and goes
            idle until you push back.
          </li>
          <li>
            • You can push it back to any of your devices anytime from
            the workspace menu.
          </li>
        </ul>
      )}
    </>
  );
}

function HostNotConfiguredBlock({
  hostServerId,
  onClose,
}: {
  hostServerId: string | null;
  onClose: () => void;
}) {
  return (
    <div className="rounded-md border border-warning/30 bg-warning/10 px-3 py-2.5 text-[12px] text-warning/95 leading-relaxed space-y-2">
      <p>
        This workspace lives on a device you haven't configured here
        yet
        {hostServerId ? (
          <>
            {" "}
            (host id <span className="font-mono">{hostServerId}</span>).
          </>
        ) : (
          "."
        )}
      </p>
      <p className="text-warning/85">
        Add the device in Settings → Devices, then come back and pull.
      </p>
      <Button
        variant="outline"
        size="sm"
        className="h-7 px-2.5 text-[12px] gap-1.5 border-warning/30 hover:bg-warning/15"
        onClick={() => {
          onClose();
          // TODO: deep-link to Settings → Devices when that route lands.
        }}
      >
        <Settings2 className="size-3" />
        Open settings
      </Button>
    </div>
  );
}

function CloneFallbackBlock({
  syncRow,
  preview,
}: {
  syncRow: WorkspaceSyncView;
  preview: AdoptionPreview;
}) {
  return (
    <>
      <p className="text-[12px] text-muted-foreground/85 leading-relaxed">
        This workspace lives only on another device (no shared host).
        We'll clone it from git.
      </p>

      <dl className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[13px] space-y-1.5">
        <SummaryRow label="Clone from">
          <span className="font-mono text-[12px] break-all">
            {syncRow.project_remote ?? "—"}
          </span>
        </SummaryRow>
        <SummaryRow
          label="Branch"
          icon={<GitBranch className="size-3 text-muted-foreground/70" />}
        >
          <span className="font-mono text-[12px]">
            {syncRow.git_branch ?? "main"}
          </span>
        </SummaryRow>
        <SummaryRow label="Will land">
          <span className="font-mono text-[11px] text-muted-foreground/80 break-all">
            {preview.suggested_path}
          </span>
        </SummaryRow>
      </dl>

      <div className="flex items-start gap-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-[12px] text-warning/90 leading-relaxed">
        <AlertTriangle className="size-3.5 shrink-0 mt-0.5" />
        <div>
          <p className="font-medium text-warning">
            Uncommitted work on the other device will NOT come over.
          </p>
          <p className="text-warning/80">
            Only committed history clones. If you want the in-flight
            edits, open the other device first and commit (or push
            the workspace to a shared device instead, then pull from
            here).
          </p>
        </div>
      </div>
    </>
  );
}

function NoOptionsBlock() {
  return (
    <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12px] text-muted-foreground/85 leading-relaxed">
      We don't have a way to bring this workspace over yet — no shared
      host, no git remote URL. Open the device it lives on and push
      it to a shared device first, then pull from here.
    </div>
  );
}

function PathInUseBlock({ path }: { path: string }) {
  return (
    <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-[12px] text-destructive/95 leading-relaxed">
      <p className="mb-1">A different workspace is already using:</p>
      <p className="font-mono text-[11px] text-destructive/85 break-all">
        {path}
      </p>
      <p className="mt-2 text-destructive/85">
        Close that workspace first, or wait for the upcoming "choose
        different path" picker.
      </p>
    </div>
  );
}

function SameBranchProjectBlock({
  onOpenExisting,
}: {
  onOpenExisting: () => void;
}) {
  // Quieter than PathInUseBlock (which is a destructive error
  // state — "this path is occupied"), softer than AlreadyAdoptedBlock
  // (which says "you literally pulled this same row before"). This
  // is "you have a logically-equivalent workspace open — same
  // branch of the same project — just at a different path."
  return (
    <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12px] text-muted-foreground/85 leading-relaxed space-y-2">
      <p>
        You already have this branch open on this device, just at a
        different path.
      </p>
      <p className="text-muted-foreground/65">
        Pulling would create a parallel copy of work you're already
        doing. Open the existing workspace instead.
      </p>
      <Button
        variant="secondary"
        size="sm"
        className="h-7 px-2.5 text-[12px]"
        onClick={onOpenExisting}
      >
        Open the existing workspace
      </Button>
    </div>
  );
}

function AlreadyAdoptedBlock({ onOpen }: { onOpen: () => void }) {
  return (
    <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12px] text-muted-foreground/85 leading-relaxed space-y-2">
      <p>You've already pulled this workspace to this device.</p>
      <Button
        variant="secondary"
        size="sm"
        className="h-7 px-2.5 text-[12px]"
        onClick={onOpen}
      >
        Open it
      </Button>
    </div>
  );
}

function ErrorBlock({ message }: { message: string }) {
  return (
    <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-[12px] text-destructive/95">
      {message}
    </div>
  );
}

function SummaryRow({
  label,
  icon,
  children,
}: {
  label: string;
  icon?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline gap-2">
      <dt className="w-[68px] shrink-0 text-[11px] uppercase tracking-wider text-muted-foreground/55">
        {label}
      </dt>
      <dd
        className={cn(
          "min-w-0 flex-1 text-[12px] text-foreground/90 flex items-center gap-1",
        )}
      >
        {icon}
        <span className="min-w-0 truncate">{children}</span>
      </dd>
    </div>
  );
}
