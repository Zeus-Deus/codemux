import { Folder, GitBranch, Home } from "lucide-react";

import {
  PrStatusIcon,
  humanizePrState,
  normalizePrState,
  prStatusTextClass,
} from "@/components/github/pr-status-icon";
import { openExternalUrl } from "@/lib/open-url";
import { basename } from "@/lib/path";
import { providerForWorkspace, providerRef } from "@/lib/source-control";
import { cn } from "@/lib/utils";
import { useActiveWorkspace } from "@/stores/app-store";

import { SCOPE_STRIP, SCOPE_STRIP_INSET } from "./pickers/ThreadScopeRow";

interface Props {
  projectRoot: string;
  isHomeWorkspace: boolean;
  branch: string | null;
}

/**
 * The strip tucked under a new chat's composer: where the first message
 * will run (project · branch) and the branch's pull request, if any.
 *
 * Orientation only, so it is shown until the chat starts and then goes
 * away. Everything a running chat needs about its workspace (git state,
 * PR watch, issue, live browser) lives in the title bar's Workspace
 * details card instead, and the composer's own controls row keeps the
 * model, effort and access pickers.
 */
export function ComposerContextRow({ projectRoot, isHomeWorkspace, branch }: Props) {
  return (
    <div className={SCOPE_STRIP_INSET}>
      <div
        data-testid="composer-context-row"
        className={cn(SCOPE_STRIP, "gap-2")}
        data-mobile-scope-strip
      >
        <div className="flex min-w-0 items-center gap-0.5 text-label font-medium text-muted-foreground">
          <span
            className="inline-flex h-6 min-w-0 shrink-0 items-center gap-1.5 rounded-md px-2"
            title={projectRoot}
          >
            {isHomeWorkspace ? (
              <Home className="size-3.5 shrink-0 text-status-remote" />
            ) : (
              <Folder className="size-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="max-w-[140px] truncate">
              {isHomeWorkspace ? "Home" : basename(projectRoot)}
            </span>
          </span>
          {branch && (
            <>
              <span className="shrink-0 select-none text-muted-foreground/50">·</span>
              <span
                className="inline-flex h-6 min-w-0 items-center gap-1.5 rounded-md px-2 font-mono"
                title={`Branch: ${branch}`}
              >
                <GitBranch className="size-3 shrink-0 text-muted-foreground" />
                <span className="truncate">{branch}</span>
              </span>
            </>
          )}
        </div>
        <PullRequestChip />
      </div>
    </div>
  );
}

/** The branch's change request, opened on its hosting product. */
function PullRequestChip() {
  const workspace = useActiveWorkspace();
  const prState = normalizePrState(workspace?.pr_state);
  if (!workspace || !prState) return null;
  const provider = providerForWorkspace(workspace);
  const ref = workspace.pr_number
    ? providerRef(provider, workspace.pr_number)
    : provider.shortNoun;
  return (
    <button
      type="button"
      disabled={!workspace.pr_url}
      onClick={(event) => {
        if (workspace.pr_url) void openExternalUrl(workspace.pr_url, { event });
      }}
      title={`${provider.nounTitle} ${ref} — ${humanizePrState(workspace.pr_state) ?? ""}`}
      className={cn(
        "ml-auto inline-flex h-6 shrink-0 items-center gap-1 rounded-md px-1.5 font-mono text-label font-semibold transition-colors duration-150 hover:enabled:bg-surface-2 disabled:opacity-60",
        prStatusTextClass(prState),
      )}
    >
      <PrStatusIcon state={prState} size={3.5} />
      {ref}
    </button>
  );
}
