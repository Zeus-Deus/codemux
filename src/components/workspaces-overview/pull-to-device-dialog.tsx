import { useCallback, useEffect, useState } from "react";

import {
  ArrowDownToLine,
  ChevronDown,
  ChevronRight,
  Cloud,
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
  workspacesAdoptionPreview,
  workspacesAdoptSynced,
  type AdoptionPreview,
  type WorkspaceSyncView,
} from "@/tauri/commands";

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

  const handleSubmit = useCallback(async () => {
    if (!syncRow || !serverId || !preview) return;
    setSubmitting(true);
    // Optimistic: signal the in-flight state immediately so the
    // sidebar + overview show a spinner the moment the dialog
    // closes. We don't have a local workspace id yet (the shell is
    // created inside `workspaces_adopt_synced`), so we key the
    // in-flight signal on the server id prefixed to avoid colliding
    // with real workspace ids.
    const inFlightKey = `pending-adopt-${serverId}`;
    setPushPullInFlight(inFlightKey);
    onOpenChange(false);

    try {
      const result = await workspacesAdoptSynced(serverId);
      markFirstPullSeen();
      // Nudge the synced-rows cache so the row migrates from
      // sibling-device to local in the overview without waiting
      // for the 5s polling tick.
      void refreshSync();
      toast.success(`Pulled ${syncRow.title} to this device`, {
        description:
          preview.host_label
            ? `From ${preview.host_label}. The copy on ${preview.host_label} stays in place — push back from this device whenever you want.`
            : result.message,
        action: {
          label: "Open",
          onClick: () => {
            setShowWorkspacesOverview(false);
            void activateWorkspace(result.workspace_id);
          },
        },
      });
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
    setPushPullInFlight,
    onOpenChange,
    refreshSync,
    setShowWorkspacesOverview,
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
          ) : !preview.host_configured ? (
            <HostNotConfiguredBlock
              hostServerId={syncRow.host_server_id}
              onClose={() => onOpenChange(false)}
            />
          ) : !preview.can_host_adopt ? (
            <CloneFallbackComingSoonBlock />
          ) : preview.is_path_in_use ? (
            <PathInUseBlock path={preview.suggested_path} />
          ) : (
            <HostBackedAdoptionForm
              syncRow={syncRow}
              preview={preview}
              disclosureOpen={disclosureOpen}
              onToggleDisclosure={() =>
                setDisclosureOpen((open) => !open)
              }
            />
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
              preview.host_configured &&
              preview.can_host_adopt &&
              !preview.is_path_in_use && (
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
                  Pull workspace
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
      <dl className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12.5px] space-y-1.5">
        <SummaryRow
          label="From"
          icon={<Cloud className="size-3 text-sky-400/80" />}
        >
          {preview.host_label ?? "—"}
        </SummaryRow>
        <SummaryRow label="Project">
          {syncRow.project_path
            ? syncRow.project_path.split("/").filter(Boolean).slice(-1)[0] ??
              "—"
            : "—"}
        </SummaryRow>
        <SummaryRow label="Branch">
          <span className="font-mono text-[11.5px]">
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
        className="flex w-full items-center gap-1 text-[11.5px] text-muted-foreground/75 hover:text-foreground transition-colors"
      >
        {disclosureOpen ? (
          <ChevronDown className="size-3" />
        ) : (
          <ChevronRight className="size-3" />
        )}
        What this does
      </button>

      {disclosureOpen && (
        <ul className="rounded-md bg-muted/20 border border-border/40 px-3 py-2 text-[11.5px] text-muted-foreground/85 leading-relaxed space-y-1">
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
        className="h-7 px-2.5 text-[11.5px] gap-1.5 border-warning/30 hover:bg-warning/15"
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

function CloneFallbackComingSoonBlock() {
  return (
    <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12px] text-muted-foreground/85 leading-relaxed">
      This workspace lives only on another device (no shared host).
      Cloning from the git remote is coming in a follow-up — for now,
      open the other device and push this workspace to one of your
      devices, then pull it from there.
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

function AlreadyAdoptedBlock({ onOpen }: { onOpen: () => void }) {
  return (
    <div className="rounded-md border border-border/60 bg-muted/30 px-3 py-2.5 text-[12px] text-muted-foreground/85 leading-relaxed space-y-2">
      <p>You've already pulled this workspace to this device.</p>
      <Button
        variant="secondary"
        size="sm"
        className="h-7 px-2.5 text-[11.5px]"
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
      <dt className="w-[68px] shrink-0 text-[10.5px] uppercase tracking-wider text-muted-foreground/55">
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
