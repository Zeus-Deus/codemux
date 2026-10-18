import { useEffect, useMemo, useState } from "react";

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
import { Eyebrow } from "@/components/ui/eyebrow";
import { Spinner } from "@/components/ui/spinner";
import { errorMessage, toast } from "@/lib/toast";
import {
  worktreeCleanupRemove,
  worktreeCleanupScan,
  type StaleWorktree,
} from "@/tauri/commands";

import {
  closeWorktreeCleanup,
  pluralWorktrees,
  staleReasonLabel,
  useWorktreeCleanupStore,
} from "./worktree-cleanup-store";

type Scan =
  | { state: "loading" }
  | { state: "failed"; message: string }
  | { state: "ready"; worktrees: StaleWorktree[] };

function WorktreeRow({
  worktree,
  checked,
  onCheckedChange,
}: {
  worktree: StaleWorktree;
  /** Undefined for a kept worktree, which can't be selected. */
  checked?: boolean;
  onCheckedChange?: (checked: boolean) => void;
}) {
  const kept = worktree.kept_because !== null;
  return (
    <label
      className={
        kept
          ? "flex items-start gap-2.5 px-3 py-2"
          : "flex cursor-pointer items-start gap-2.5 px-3 py-2 select-none hover:bg-surface-2/60"
      }
      title={worktree.path}
    >
      {!kept && (
        <Checkbox
          className="mt-0.5"
          checked={checked}
          onCheckedChange={(next) => onCheckedChange?.(next === true)}
          aria-label={`Remove ${worktree.title}`}
        />
      )}
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-body font-medium text-foreground">
            {worktree.title}
          </span>
          {worktree.branch && worktree.branch !== worktree.title && (
            <span className="max-w-44 shrink-0 truncate rounded-sm bg-muted px-1.5 font-mono text-caption leading-[16px] text-muted-foreground">
              {worktree.branch}
            </span>
          )}
        </div>
        <p className="truncate text-label text-muted-foreground">
          {kept ? worktree.kept_because : staleReasonLabel(worktree)}
          <span className="text-muted-foreground/60">
            {" · "}
            {worktree.project ? `${worktree.project} · ` : ""}
            {worktree.source}
          </span>
        </p>
      </div>
    </label>
  );
}

/**
 * Review for stale worktrees: what would be removed and why, plus the stale
 * ones that stay because they hold work. Nothing is removed until the user
 * confirms; the backend re-checks each worktree right before removing it.
 */
export function WorktreeCleanupDialog() {
  const open = useWorktreeCleanupStore((s) => s.open);
  const [scan, setScan] = useState<Scan>({ state: "loading" });
  const [unchecked, setUnchecked] = useState<ReadonlySet<string>>(new Set());
  const [removing, setRemoving] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setScan({ state: "loading" });
    setUnchecked(new Set());
    worktreeCleanupScan()
      .then((worktrees) => {
        if (!cancelled) setScan({ state: "ready", worktrees });
      })
      .catch((err: unknown) => {
        if (!cancelled) setScan({ state: "failed", message: errorMessage(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const { removable, kept } = useMemo(() => {
    const worktrees = scan.state === "ready" ? scan.worktrees : [];
    return {
      removable: worktrees.filter((w) => w.kept_because === null),
      kept: worktrees.filter((w) => w.kept_because !== null),
    };
  }, [scan]);
  const selected = removable.filter((w) => !unchecked.has(w.path));

  const toggle = (path: string, checked: boolean) =>
    setUnchecked((prev) => {
      const next = new Set(prev);
      if (checked) next.delete(path);
      else next.add(path);
      return next;
    });

  const handleRemove = async () => {
    setRemoving(true);
    try {
      const outcome = await worktreeCleanupRemove(selected.map((w) => w.path));
      if (outcome.removed.length > 0) {
        toast.success(`Removed ${pluralWorktrees(outcome.removed.length)}`, {
          description: "Their branches and commits were kept.",
        });
      }
      if (outcome.kept.length > 0) {
        toast.warning(`Kept ${pluralWorktrees(outcome.kept.length)}`, {
          description: outcome.kept
            .map((w) => `${w.title}: ${w.kept_because}`)
            .join("\n"),
        });
      }
      closeWorktreeCleanup();
    } catch (err) {
      toast.failure("Worktree cleanup failed", err);
    } finally {
      setRemoving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && !removing && closeWorktreeCleanup()}>
      <DialogContent className="sm:max-w-[460px]" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle className="text-body-lg">Clean up stale worktrees</DialogTitle>
          <DialogDescription className="text-body leading-relaxed">
            Removes these checkouts from disk. Branches and their commits are
            kept, so archived workspaces can still be restored.
          </DialogDescription>
        </DialogHeader>

        {scan.state === "loading" && (
          <div className="flex items-center gap-2 py-6 text-body text-muted-foreground">
            <Spinner className="size-3.5" />
            Checking worktrees…
          </div>
        )}
        {scan.state === "failed" && (
          <p className="py-4 text-body text-destructive">{scan.message}</p>
        )}
        {scan.state === "ready" && removable.length === 0 && kept.length === 0 && (
          <p className="py-4 text-body leading-relaxed text-muted-foreground">
            No stale worktrees. A worktree shows up here once its branch or
            pull request merges, or 30 days after its workspace was archived
            or closed.
          </p>
        )}
        {scan.state === "ready" && (removable.length > 0 || kept.length > 0) && (
          <div className="-mx-1 max-h-[340px] space-y-4 overflow-y-auto px-1">
            {removable.length > 0 && (
              <div className="divide-y divide-border/50 overflow-hidden rounded-lg border border-border/60 bg-muted/30">
                {removable.map((w) => (
                  <WorktreeRow
                    key={w.path}
                    worktree={w}
                    checked={!unchecked.has(w.path)}
                    onCheckedChange={(checked) => toggle(w.path, checked)}
                  />
                ))}
              </div>
            )}
            {kept.length > 0 && (
              <section>
                <Eyebrow className="mb-2">Kept · holds work</Eyebrow>
                <div className="divide-y divide-border/40 overflow-hidden rounded-lg border border-border/40 opacity-80">
                  {kept.map((w) => (
                    <WorktreeRow key={w.path} worktree={w} />
                  ))}
                </div>
              </section>
            )}
          </div>
        )}

        <DialogFooter className="gap-2 sm:gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={removing}
            onClick={closeWorktreeCleanup}
          >
            {removable.length === 0 ? "Close" : "Cancel"}
          </Button>
          {removable.length > 0 && (
            <Button
              type="button"
              variant="destructive"
              size="sm"
              disabled={selected.length === 0 || removing}
              onClick={() => void handleRemove()}
            >
              {removing && <Spinner className="size-3.5" />}
              Remove {pluralWorktrees(selected.length)}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
