import { useMemo, useRef, useState } from "react";

import { AlertTriangle, Archive, ArchiveRestore, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { relativeTime } from "@/lib/relative-time";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import {
  deleteArchivedWorkspace,
  unarchiveWorkspace,
} from "@/tauri/commands";
import type { ArchivedWorkspaceSnapshot } from "@/tauri/types";
import {
  projectDisplayName,
  resolveProjectRoot,
  useAppStore,
  useHomeDir,
} from "@/stores/app-store";
import { useForceDelete } from "@/hooks/use-force-delete";

/**
 * Settings → Archive.
 *
 * Lists every archived workspace grouped by project. Archiving is
 * non-destructive — files, branches, and worktrees stay on disk — so
 * each entry offers:
 *   - Unarchive: restore the workspace to the sidebar (backend also
 *     activates it).
 *   - Delete…: drop the archive entry, optionally deleting the worktree
 *     (and branch) from disk. A dirty worktree rejects with a message
 *     matching /use force/i; the dialog escalates in place to a
 *     "Force delete" button instead of closing (same state machine as
 *     the sidebar's delete dialog).
 *   - Protected repo-root entries only ever offer "Remove from archive"
 *     — the entry disappears, the files are never touched.
 */

// Stable empty-array fallback so the zustand selector doesn't hand out a
// fresh reference (→ re-render) on every snapshot tick without the field.
const NO_ARCHIVED: ArchivedWorkspaceSnapshot[] = [];

const STALE_AFTER_SECONDS = 30 * 86_400;

interface ProjectGroup {
  path: string;
  label: string;
  entries: ArchivedWorkspaceSnapshot[];
}

/**
 * Subscribe to the archived-workspace list without churning on every
 * app-state emit. The backend replaces `appState` wholesale many times
 * a second (token streaming, git polls, …), so a naive
 * `s.appState?.archived_workspaces` selector would re-render this panel
 * on every tick. Same memoized-selector pattern as zustand's
 * `useShallow`, but with archive-list equality: the previous array is
 * kept whenever the archive_id sequence (length + ids) is unchanged —
 * archive entries are immutable once created, so id-identity is
 * content-identity.
 */
function useArchivedWorkspaces(): ArchivedWorkspaceSnapshot[] {
  const prevRef = useRef<ArchivedWorkspaceSnapshot[]>(NO_ARCHIVED);
  return useAppStore((s) => {
    const next = s.appState?.archived_workspaces ?? NO_ARCHIVED;
    const prev = prevRef.current;
    const sameIds =
      prev === next ||
      (prev.length === next.length &&
        next.every((e, i) => e.archive_id === prev[i].archive_id));
    if (!sameIds) prevRef.current = next;
    return prevRef.current;
  });
}

function isRootEntry(entry: ArchivedWorkspaceSnapshot): boolean {
  return entry.protected || entry.workspace_kind === "main";
}

/** Delete / remove-entry confirmation for one archive entry.
 *
 *  For protected roots the backend refuses worktree deletion, so the
 *  dialog offers entry removal only — no checkboxes, files untouched.
 *  For worktree entries the first Delete goes out with
 *  `forceDelete: false`; a dirty-worktree rejection matching /use force/i
 *  keeps the dialog open, shows the backend's message verbatim, and
 *  flips the button to "Force delete" (which reissues with
 *  `forceDelete: true`). Any other error → toast + close. */
function DeleteArchivedDialog({
  entry,
  open,
  onOpenChange,
}: {
  entry: ArchivedWorkspaceSnapshot;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const isRoot = isRootEntry(entry);
  const hasWorktree = !isRoot && entry.worktree_path !== null;
  const [deleteWorktree, setDeleteWorktree] = useState(false);
  const [deleteBranch, setDeleteBranch] = useState(true);

  const { forceMessage, confirm, reset } = useForceDelete({
    run: (force) =>
      deleteArchivedWorkspace(
        entry.archive_id,
        hasWorktree && deleteWorktree,
        hasWorktree && deleteWorktree && deleteBranch,
        force,
      ),
    onDone: () => {
      toast.success(
        isRoot
          ? `Removed "${entry.title}" from the archive`
          : `Deleted "${entry.title}"`,
      );
      handleOpenChange(false);
    },
    onError: (message) => {
      toast.error("Delete failed", { description: message });
      handleOpenChange(false);
    },
  });

  function handleOpenChange(next: boolean) {
    if (!next) {
      reset();
      setDeleteWorktree(false);
      setDeleteBranch(true);
    }
    onOpenChange(next);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-[360px]">
        <DialogHeader>
          <DialogTitle className="text-sm">
            {isRoot
              ? <>Remove &ldquo;{entry.title}&rdquo; from archive?</>
              : <>Delete &ldquo;{entry.title}&rdquo;?</>}
          </DialogTitle>
          <DialogDescription>
            {isRoot
              ? "Removes this entry from the archive. The repository files on disk are never touched."
              : "Removes this entry from the archive. Optionally delete its worktree directory from disk as well."}
          </DialogDescription>
        </DialogHeader>

        {forceMessage !== null && (
          <div className="flex items-center gap-2 rounded-md border border-status-working/20 bg-status-working/10 px-2.5 py-1.5 text-xs text-status-working">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            {forceMessage}
          </div>
        )}

        {hasWorktree && (
          <div className="space-y-2">
            <label className="flex items-center gap-2 cursor-pointer select-none">
              <input
                type="checkbox"
                checked={deleteWorktree}
                onChange={(e) => setDeleteWorktree(e.target.checked)}
                className="rounded border-border"
              />
              <span className="text-xs text-muted-foreground">
                Also delete worktree from disk
              </span>
            </label>
            {deleteWorktree && (
              <label className="ml-6 flex items-center gap-2 cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={deleteBranch}
                  onChange={(e) => setDeleteBranch(e.target.checked)}
                  className="rounded border-border"
                />
                <span className="text-xs text-muted-foreground">
                  Also delete local branch
                </span>
              </label>
            )}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={() => handleOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            variant={isRoot ? "secondary" : "destructive"}
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={() => void confirm()}
          >
            {forceMessage !== null
              ? "Force delete"
              : isRoot
                ? "Remove"
                : "Delete"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ArchivedEntryRow({
  entry,
  onDeleteRequest,
}: {
  entry: ArchivedWorkspaceSnapshot;
  onDeleteRequest: () => void;
}) {
  const [unarchiving, setUnarchiving] = useState(false);
  const isRoot = isRootEntry(entry);
  const archivedDate = new Date(entry.archived_at * 1000);
  const isStale =
    Date.now() / 1000 - entry.archived_at > STALE_AFTER_SECONDS;

  const handleUnarchive = async () => {
    setUnarchiving(true);
    try {
      await unarchiveWorkspace(entry.archive_id);
      toast.success(`Restored "${entry.title}"`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Unarchive failed", { description: message });
    } finally {
      setUnarchiving(false);
    }
  };

  return (
    <div className="flex items-center justify-between gap-4 rounded-lg border border-border/60 bg-muted/30 px-3.5 py-2.5">
      <div className="min-w-0 space-y-1">
        <div className="flex items-center gap-2 min-w-0">
          <span className="truncate text-[13px] font-medium text-foreground">
            {entry.title}
          </span>
          {isRoot && (
            <span className="shrink-0 rounded px-1.5 text-[10px] leading-[16px] text-muted-foreground bg-muted border border-border/60">
              repo root
            </span>
          )}
          {entry.git_branch && (
            <span className="shrink-0 truncate max-w-48 rounded bg-muted px-1.5 font-mono text-[10px] leading-[16px] text-muted-foreground">
              {entry.git_branch}
            </span>
          )}
          {isStale && (
            <span
              className="shrink-0 rounded px-1.5 text-[10px] leading-[16px] text-muted-foreground/60 bg-muted/60"
              title="Archived more than 30 days ago"
            >
              stale
            </span>
          )}
        </div>
        <p className="truncate text-[11px] text-muted-foreground/70">
          Archived {relativeTime(archivedDate)}
        </p>
      </div>

      <div className="flex items-center gap-1.5 shrink-0">
        <Button
          variant="secondary"
          size="sm"
          className="h-7 px-2.5 text-xs"
          disabled={unarchiving}
          onClick={() => void handleUnarchive()}
        >
          <ArchiveRestore className="h-3.5 w-3.5" />
          Unarchive
        </Button>
        {isRoot ? (
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2.5 text-xs text-muted-foreground hover:text-foreground"
            onClick={onDeleteRequest}
          >
            Remove from archive
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="icon-xs"
            className="text-muted-foreground hover:text-destructive"
            aria-label={`Delete archived workspace "${entry.title}"`}
            onClick={onDeleteRequest}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
    </div>
  );
}

export function ArchiveSection() {
  const archived = useArchivedWorkspaces();
  const homeDir = useHomeDir();
  const [deleteTarget, setDeleteTarget] =
    useState<ArchivedWorkspaceSnapshot | null>(null);

  // Group keys and labels come from the store's shared helpers
  // (`resolveProjectRoot` / `projectDisplayName`) so this panel names
  // projects exactly like the sidebar grouping does (same root
  // derivation, same "Home" special case).
  const groups: ProjectGroup[] = useMemo(() => {
    const byRoot = new Map<string, ArchivedWorkspaceSnapshot[]>();
    for (const entry of archived) {
      const root = resolveProjectRoot(entry);
      const list = byRoot.get(root);
      if (list) list.push(entry);
      else byRoot.set(root, [entry]);
    }
    return [...byRoot.entries()].map(([path, entries]) => ({
      path,
      label: projectDisplayName(path, homeDir),
      // Newest archives first within a project.
      entries: [...entries].sort((a, b) => b.archived_at - a.archived_at),
    }));
  }, [archived, homeDir]);

  if (archived.length === 0) {
    return (
      <div className="flex flex-col items-center gap-2 rounded-lg border border-border/60 bg-muted/30 px-4 py-10 text-center">
        <Archive className="h-5 w-5 text-muted-foreground/50" />
        <p className="text-[13px] text-muted-foreground">
          No archived workspaces
        </p>
        <p className="text-[12px] text-muted-foreground/70 max-w-prose">
          Archive a workspace from the sidebar to park it here without
          touching its files.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-8">
      {groups.map((group, idx) => (
        <section key={group.path} className={cn(idx === 0 && "mt-0")}>
          <p
            className="mb-3 font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55"
            title={group.path}
          >
            {group.label}
          </p>
          <div className="space-y-2">
            {group.entries.map((entry) => (
              <ArchivedEntryRow
                key={entry.archive_id}
                entry={entry}
                onDeleteRequest={() => setDeleteTarget(entry)}
              />
            ))}
          </div>
        </section>
      ))}

      {deleteTarget && (
        <DeleteArchivedDialog
          // Key by entry so per-open checkbox/escalation state never
          // leaks between entries.
          key={deleteTarget.archive_id}
          entry={deleteTarget}
          open
          onOpenChange={(next) => {
            if (!next) setDeleteTarget(null);
          }}
        />
      )}
    </div>
  );
}
