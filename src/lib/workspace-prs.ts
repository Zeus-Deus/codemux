import {
  isPrOnCurrentBranch,
  normalizePrState,
  type PrStatusState,
} from "@/components/github/pr-status-icon";
import type { WorkspacePrRef, WorkspaceSnapshot } from "@/tauri/types";
import { providerRef, type ProviderPresentation } from "@/lib/source-control";

export type { WorkspacePrRef };

/** The PRs a workspace owns, primary first.
 *
 *  A workspace produces a *set* of PRs, not one — an agent handed a multi-part
 *  plan routinely lands a branch and a PR per concern. `prs` is that set; the
 *  flat `pr_*` scalars are its head, kept because every workspace-scoped
 *  surface already reads them.
 *
 *  The scalar fallback is not dead code: it covers a snapshot persisted before
 *  `prs` existed, and any provider whose set discovery is the default
 *  single-PR delegation. Both cases mean "one PR, described by the scalars". */
export function workspacePrs(
  workspace: Pick<
    WorkspaceSnapshot,
    "prs" | "pr_number" | "pr_state" | "pr_url" | "pr_head_branch"
  >,
): WorkspacePrRef[] {
  if (workspace.prs && workspace.prs.length > 0) return workspace.prs;
  if (workspace.pr_number == null || !workspace.pr_state) return [];
  return [
    {
      number: workspace.pr_number,
      state: workspace.pr_state,
      url: workspace.pr_url ?? "",
      head_branch: workspace.pr_head_branch ?? null,
    },
  ];
}

export interface PrSetSummary {
  total: number;
  open: number;
  draft: number;
  merged: number;
  closed: number;
  /** The state the badge should show for the set as a whole. */
  state: PrStatusState | null;
  /** Every PR has reached a terminal state (merged or closed). */
  allSettled: boolean;
}

/** Collapse a set of PRs into the one state a badge can show.
 *
 *  Precedence is outstanding-work-first: `open` > `draft` > `merged` >
 *  `closed`. A stack whose first four PRs merged while five are still open is
 *  an *open* piece of work — reporting it as merged would settle the card out
 *  of the sidebar with most of the review still ahead of it. `draft` outranks
 *  the terminal states for the same reason and sits below `open` because a
 *  draft is work the author has declared unfinished.
 *
 *  `closed` only wins when nothing merged at all, so a stack that partly
 *  landed and partly got abandoned reads as merged rather than as discarded. */
export function prSetSummary(prs: readonly WorkspacePrRef[]): PrSetSummary {
  let open = 0;
  let draft = 0;
  let merged = 0;
  let closed = 0;
  for (const pr of prs) {
    switch (normalizePrState(pr.state)) {
      case "open":
        open += 1;
        break;
      case "draft":
        draft += 1;
        break;
      case "merged":
        merged += 1;
        break;
      case "closed":
        closed += 1;
        break;
      default:
        break;
    }
  }
  const total = open + draft + merged + closed;
  const state: PrStatusState | null =
    open > 0
      ? "open"
      : draft > 0
        ? "draft"
        : merged > 0
          ? "merged"
          : closed > 0
            ? "closed"
            : null;
  return {
    total,
    open,
    draft,
    merged,
    closed,
    state,
    allSettled: total > 0 && open === 0 && draft === 0,
  };
}

/** May lifecycle rules act on this workspace's PRs?
 *
 *  A single association can be the backend's weaker side-branch badge — a PR
 *  opened from a branch this worktree merely visited — and settling a checkout
 *  on the strength of that is wrong, so it still goes through
 *  `isPrOnCurrentBranch`.
 *
 *  A set of more than one never can be. The side-branch fallback contributes at
 *  most one PR and only when nothing else was found, so anything larger came
 *  from worktree-owned discovery, where every branch is by construction
 *  reachable from this checkout's own HEAD. Those are this workspace's own
 *  commits regardless of which branch is checked out right now, which is
 *  exactly the stack case: the agent cut nine branches and left HEAD on none of
 *  them. */
export function prsDescribeThisCheckout(
  prs: readonly WorkspacePrRef[],
  gitBranch: string | null | undefined,
): boolean {
  if (prs.length > 1) return true;
  return isPrOnCurrentBranch(prs[0]?.head_branch, gitBranch);
}

/** Is this PR stacked on another PR in the same set?
 *
 *  Stacking is not a thing the app records — it is a shape GitHub already
 *  holds, visible whenever one PR's base branch is another's head. Reading it
 *  back this way means a stack the agent built with plain `gh` is understood
 *  without the agent having to declare anything. */
export function stackedOn(
  pr: WorkspacePrRef,
  prs: readonly WorkspacePrRef[],
): WorkspacePrRef | null {
  if (!pr.base_branch) return null;
  return (
    prs.find(
      (other) =>
        other.number !== pr.number && other.head_branch === pr.base_branch,
    ) ?? null
  );
}

/** Accessible name for the sidebar's PR badge.
 *
 *  A set gets its composition read out rather than its head alone, because the
 *  composition is the whole reason the badge is there: "nine pull requests —
 *  four merged, five open" is the sentence a count is standing in for, and a
 *  screen-reader user should not have to open the workspace to hear it. */
export function prSetLabel(
  provider: ProviderPresentation,
  prs: readonly WorkspacePrRef[],
  summary: PrSetSummary,
  /** Effective badge state. Normally `summary.state`; callers pass their own
   *  when they render a state the set cannot describe — a stored state with
   *  no PR number, which the badge shows as a bare icon. */
  state: PrStatusState | null = summary.state,
): string {
  if (summary.total <= 1) {
    const primary = prs[0];
    return primary
      ? `${provider.nounTitle} ${providerRef(provider, primary.number)} — ${state}`
      : `${provider.nounTitle} — ${state}`;
  }
  const parts: string[] = [];
  if (summary.open > 0) parts.push(`${summary.open} open`);
  if (summary.draft > 0) parts.push(`${summary.draft} draft`);
  if (summary.merged > 0) parts.push(`${summary.merged} merged`);
  if (summary.closed > 0) parts.push(`${summary.closed} closed`);
  const primary = prs[0];
  const opens = primary
    ? `. Opens ${providerRef(provider, primary.number)}`
    : "";
  return `${summary.total} ${provider.nounPlural} — ${parts.join(", ")}${opens}`;
}
