import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MoreHorizontal } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { normalizePrState } from "@/components/github/pr-status-icon";
import type { PullRequestInfo } from "@/tauri/types";
import type { ProviderPresentation } from "@/lib/source-control";
import {
  groupDigits,
  relativeAge,
  tzBody,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
  tzPanelTitle,
} from "./review-ui";

/** State pill tones, matching the sidebar PR icon so one PR reads the
 *  same colour everywhere it appears. */
const STATE_PILL: Record<string, { label: string; className: string }> = {
  open: { label: "Open", className: "bg-status-open/12 text-status-open" },
  draft: { label: "Draft", className: "bg-muted text-muted-foreground" },
  merged: { label: "Merged", className: "bg-accent-violet/12 text-accent-violet" },
  closed: { label: "Closed", className: "bg-destructive/12 text-destructive" },
};

/**
 * The header's branch control, for surfaces that are not standing in
 * the branch — i.e. the Pull Requests page.
 *
 * The panel doesn't pass one: it is nearly always already in the branch,
 * and a Check out button there would offer to do something that is
 * already done.
 */
export interface CheckoutControl {
  /** A workspace already has this branch: Switch to it rather than
   *  cutting a second worktree for the same branch. */
  hasWorkspace: boolean;
  onCheckOut: () => void;
  onCopyBranch: () => void;
  busy?: boolean;
}

interface Props {
  pr: PullRequestInfo;
  provider: ProviderPresentation;
  /** `owner/name`, when the checkout could name it. */
  repoSlug: string | null;
  /** True when the workspace is standing in the PR's head branch — the
   *  panel says so instead of offering a Check out button. */
  checkedOutHere: boolean;
  checkout?: CheckoutControl;
  onRefresh: () => void;
}

export function ReviewHeader({
  pr,
  provider,
  repoSlug,
  checkedOutHere,
  checkout,
  onRefresh,
}: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const state = pr.is_draft && pr.state === "OPEN" ? "draft" : normalizePrState(pr.state) ?? "open";
  const pill = STATE_PILL[state] ?? STATE_PILL.open;
  const age = relativeAge(pr.updated_at);

  const openInBrowser = () => {
    openUrl(pr.url).catch((err) => toast.error(String(err)));
  };

  const copyUrl = () => {
    navigator.clipboard
      .writeText(pr.url)
      .then(() => toast.success("Link copied"))
      .catch(() => toast.error("Couldn't copy the link"));
  };

  return (
    // Border runs full-bleed; the content shares the body's centered
    // reading column so title, prose and controls sit on one axis.
    <div className="border-b border-border/40" data-testid="review-header">
    <div className="mx-auto flex w-full max-w-[920px] flex-col gap-2 px-3.5 pb-2.5 pt-3">
      <div className="flex items-center gap-1.5">
        {repoSlug && (
          <span className={cn("truncate text-muted-foreground", tzMetaNum)}>{repoSlug}</span>
        )}
        <span className={cn("shrink-0 font-mono text-foreground/70", tzMetaNum)}>
          {provider.sigil}
          {pr.number}
        </span>
        <span
          className={cn(
            "shrink-0 rounded px-1.5 py-[3px] font-semibold",
            tzMeta,
            pill.className,
          )}
          data-testid="pr-state-pill"
        >
          {pill.label}
        </span>
        <span className="flex-1" />
        {!checkedOutHere && checkout?.hasWorkspace && (
          <span
            className={cn("shrink-0 text-status-open", tzMeta)}
            data-testid="checked-out-elsewhere"
          >
            checked out
          </span>
        )}
        {!checkedOutHere && checkout && (
          <span className="flex shrink-0 items-center overflow-hidden rounded-md bg-card">
            {/* One click still checks out. The caret is a second target
                rather than a wrapper around the first, so the common
                action never costs two clicks. */}
            <button
              type="button"
              className={cn(
                "h-[30px] border-0 pl-2.5 pr-1.5 text-foreground/90 transition-colors hover:bg-accent/50 disabled:opacity-60",
                tzBody,
              )}
              data-testid="detail-checkout"
              disabled={checkout.busy}
              onClick={checkout.onCheckOut}
            >
              {checkout.hasWorkspace ? "Switch" : "Check out"}
            </button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label="Check out options"
                  className={cn(
                    "h-[30px] border-0 pl-0.5 pr-2 text-muted-foreground transition-colors hover:bg-accent/50",
                    tzEyebrow,
                  )}
                >
                  ▾
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="min-w-44">
                <DropdownMenuItem onSelect={checkout.onCheckOut}>
                  {checkout.hasWorkspace
                    ? "Switch to its workspace"
                    : "Check out in a worktree"}
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={openInBrowser}>
                  Open in browser
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={checkout.onCopyBranch}
                  disabled={!pr.head_branch}
                >
                  Copy branch name
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </span>
        )}
        <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Pull request actions"
              className="flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
            >
              <MoreHorizontal className="size-3.5" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-40">
            <DropdownMenuItem onSelect={openInBrowser}>Open in browser</DropdownMenuItem>
            <DropdownMenuItem onSelect={copyUrl}>Copy URL</DropdownMenuItem>
            <DropdownMenuItem onSelect={onRefresh}>Refresh</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {/* Two lines maximum: past that a title stops being a title and
          starts pushing the checks below the fold. */}
      <h2
        className={cn(
          "line-clamp-2 font-semibold leading-snug text-foreground",
          tzPanelTitle,
        )}
        title={pr.title}
      >
        {pr.title}
      </h2>

      <div
        className={cn(
          "flex flex-wrap items-center gap-x-2 gap-y-1 text-muted-foreground",
          tzMetaNum,
        )}
      >
        {pr.author && <span className="text-foreground/70">{pr.author}</span>}
        {age && <span>{age}</span>}
        {pr.changed_files != null && (
          <>
            <span className="opacity-40">·</span>
            <span>
              {pr.changed_files} {pr.changed_files === 1 ? "file" : "files"}
            </span>
          </>
        )}
        {pr.additions != null && (
          <span className="font-mono text-status-open">+{groupDigits(pr.additions)}</span>
        )}
        {pr.deletions != null && (
          <span className="font-mono text-destructive">−{groupDigits(pr.deletions)}</span>
        )}
      </div>

      <div
        className={cn(
          "flex min-w-0 items-center gap-1.5 font-mono text-muted-foreground",
          tzMetaNum,
        )}
      >
        <span className="shrink-0">{pr.base_branch ?? "main"}</span>
        <span className="shrink-0 opacity-60">←</span>
        {/* Truncated from the LEFT: the tail of a branch name is what
            identifies it, the `agent/` prefix is not. */}
        <span
          dir="rtl"
          className="min-w-0 flex-1 truncate text-left text-foreground/70"
          title={pr.head_branch ?? ""}
        >
          &lrm;{pr.head_branch ?? ""}
        </span>
        {checkedOutHere && (
          <span
            className={cn("shrink-0 font-sans text-status-open", tzMeta)}
            data-testid="checked-out-here"
          >
            checked out here
          </span>
        )}
      </div>
    </div>
    </div>
  );
}
