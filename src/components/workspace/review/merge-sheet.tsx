import { useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Check } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  btnCard,
  btnGreenSolid,
  tzBody,
  tzBodyLg,
  tzEyebrow,
  tzMetaNum,
  tzPanelHeader,
} from "./review-ui";
import { getMergeStrategy, setMergeStrategy } from "./pr-drafts";

export const MERGE_STRATEGIES = [
  { id: "squash", label: "Squash" },
  { id: "merge", label: "Merge" },
  { id: "rebase", label: "Rebase" },
] as const;

export interface MergeRequestPayload {
  method: string;
  deleteBranch: boolean;
  commitTitle: string;
  commitBody: string;
}

interface Props {
  open: boolean;
  prNumber: number;
  prTitle: string;
  headBranch: string | null;
  /** Prefilled body — blank when no cheap source of commit messages. */
  initialBody?: string;
  merging: boolean;
  onCancel: () => void;
  onConfirm: (payload: MergeRequestPayload) => void;
}

/**
 * The one irreversible action on this surface, so it gets a confirm —
 * but a confirm that does work rather than one that asks "are you sure".
 * Everything in it is a decision the merge needs anyway: strategy,
 * commit subject, body, and whether the branch goes with it.
 */
export function MergeSheet({
  open,
  prNumber,
  prTitle,
  headBranch,
  initialBody = "",
  merging,
  onCancel,
  onConfirm,
}: Props) {
  const [method, setMethod] = useState(() => getMergeStrategy());
  const [title, setTitle] = useState(() => `${prTitle} (#${prNumber})`);
  const [body, setBody] = useState(initialBody);
  const [deleteBranch, setDeleteBranch] = useState(true);

  const strategyLabel =
    MERGE_STRATEGIES.find((s) => s.id === method)?.label ?? "Squash";

  const pickMethod = (id: string) => {
    setMethod(id);
    setMergeStrategy(id);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !merging) onCancel();
      }}
    >
      <DialogContent showCloseButton={false} className="gap-0 p-0 sm:max-w-md">
        <div className="flex items-center gap-2 border-b border-border/40 px-3.5 pb-2.5 pt-3">
          <DialogTitle className={cn("flex-1 font-semibold", tzPanelHeader)}>
            Merge #{prNumber}
          </DialogTitle>
          <div
            className="flex gap-px rounded-lg bg-muted/60 p-0.5"
            role="radiogroup"
            aria-label="Merge strategy"
          >
            {MERGE_STRATEGIES.map((strategy) => (
              <button
                key={strategy.id}
                type="button"
                role="radio"
                aria-checked={method === strategy.id}
                onClick={() => pickMethod(strategy.id)}
                data-testid={`merge-strategy-${strategy.id}`}
                className={cn(
                  "rounded-md px-2.5 py-1.5 transition-colors",
                  tzMetaNum,
                  method === strategy.id
                    ? "bg-card font-semibold text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {strategy.label}
              </button>
            ))}
          </div>
        </div>

        <div className="flex flex-col gap-3 px-3.5 py-3">
          <label className="flex flex-col gap-1.5">
            <span
              className={cn(
                "font-mono font-semibold uppercase tracking-[0.07em] text-muted-foreground",
                tzEyebrow,
              )}
            >
              Commit title
            </span>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              data-testid="merge-commit-title"
              className={cn(
                "h-[34px] rounded-md border-0 bg-muted/50 px-2.5 text-foreground outline-none focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
                tzBodyLg,
              )}
            />
          </label>

          <label className="flex flex-col gap-1.5">
            <span
              className={cn(
                "font-mono font-semibold uppercase tracking-[0.07em] text-muted-foreground",
                tzEyebrow,
              )}
            >
              Body
            </span>
            <textarea
              value={body}
              onChange={(e) => setBody(e.target.value)}
              rows={4}
              data-testid="merge-commit-body"
              className={cn(
                "resize-y rounded-md border-0 bg-muted/50 px-2.5 py-2 leading-relaxed text-foreground outline-none focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
                tzBody,
              )}
            />
          </label>

          <button
            type="button"
            role="checkbox"
            aria-checked={deleteBranch}
            onClick={() => setDeleteBranch((v) => !v)}
            data-testid="merge-delete-branch"
            className="flex items-center gap-2 text-left"
          >
            <span
              className={cn(
                "flex size-3.5 shrink-0 items-center justify-center rounded-[3px]",
                deleteBranch
                  ? "bg-status-open/20 text-status-open ring-1 ring-status-open"
                  : "bg-muted ring-1 ring-border",
              )}
            >
              {deleteBranch && <Check className="size-2.5" strokeWidth={3.5} />}
            </span>
            <span className={cn("flex-1 text-foreground/80", tzBodyLg)}>
              Delete <span className={cn("font-mono", tzBody)}>{headBranch ?? "the branch"}</span>{" "}
              after merging
            </span>
          </button>

          <p className={cn("leading-relaxed text-muted-foreground", tzMetaNum)}>
            The worktree stays until you close it — Codemux won't remove a directory an
            agent might be running in.
          </p>
        </div>

        <div className="flex items-center gap-1.5 border-t border-border/40 bg-muted/30 px-3 py-2.5">
          <span className="flex-1" />
          <button type="button" className={btnCard} onClick={onCancel} disabled={merging}>
            Cancel
          </button>
          <button
            type="button"
            className={btnGreenSolid}
            data-testid="merge-confirm"
            onClick={() =>
              onConfirm({ method, deleteBranch, commitTitle: title, commitBody: body })
            }
            disabled={merging}
          >
            {merging ? (
              <>
                <span
                  aria-hidden
                  className="size-2.5 animate-spin rounded-full border-[1.6px] border-current border-r-transparent"
                />
                Merging
              </>
            ) : (
              `${strategyLabel} and merge`
            )}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
