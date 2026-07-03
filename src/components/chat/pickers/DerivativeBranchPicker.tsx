import { useEffect, useMemo, useState } from "react";
import { ChevronDown, FolderGit, GitBranch, Globe } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { listBranchesDetailed } from "@/tauri/commands";
import type { BranchDetail, WorkspaceSnapshot } from "@/tauri/types";

import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

// Module-scoped stable empty array — see WorktreePicker for the same
// pattern. Returning a fresh `[]` from a Zustand selector causes the
// "getSnapshot should be cached" warning.
const EMPTY_WORKSPACES: WorkspaceSnapshot[] = [];

type FilterMode = "all" | "worktrees";

function formatRelativeTime(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unixSeconds;
  if (diff < 60) return "now";
  const mins = Math.floor(diff / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  const years = Math.floor(months / 12);
  return `${years}y`;
}

interface Props {
  projectPath: string;
  value: string;
  onChange: (branch: string) => void;
  disabled?: boolean;
}

/**
 * Picks the base branch for derivative worktree creation. Sits next to
 * WorktreePicker in Zone 1 and feeds the "+ New worktree…" inline
 * submit with the branch to fork from.
 *
 * The list is fetched lazily (on first open) so idle chat panes don't
 * pay for a git call they'll never use.
 */
export function DerivativeBranchPicker({
  projectPath,
  value,
  onChange,
  disabled,
}: Props) {
  const [open, setOpen] = useState(false);
  const [branches, setBranches] = useState<BranchDetail[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [filterMode, setFilterMode] = useState<FilterMode>("all");

  // Worktrees that exist for this project — used to render the
  // FolderGit icon and power the All/Worktrees tab filter. Sourced from
  // the same app-store path as WorktreePicker so the two pills agree.
  const homeDir = useHomeDir();
  const workspaces = useAppStore(
    (s) => s.appState?.workspaces ?? EMPTY_WORKSPACES,
  );
  const groups = useProjectGroupedWorkspaces(workspaces, homeDir);
  const worktreeBranches = useMemo(() => {
    const set = new Set<string>();
    const group = groups.find((g) => g.projectPath === projectPath);
    if (!group) return set;
    for (const ws of group.workspaces) {
      if (ws.git_branch) set.add(ws.git_branch);
    }
    return set;
  }, [groups, projectPath]);

  // Fetch branches eagerly on mount — not just on popover open. The
  // seed value ("main" from the parent) is a guess that may not exist
  // in this repo (e.g. a `master`-only repo). Probing on mount lets us
  // auto-correct the default via `onChange` before the user hits Enter
  // on the "+ New worktree…" inline input.
  useEffect(() => {
    if (!projectPath) return;
    if (branches !== null) return;
    let cancelled = false;
    setLoading(true);
    listBranchesDetailed(projectPath)
      .then((rows) => {
        if (cancelled) return;
        setBranches(rows);
      })
      .catch(() => {
        if (cancelled) return;
        setBranches([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath, branches]);

  // Auto-correct the seeded default when it doesn't exist in the
  // repo. Prefer `main`, then `master`, then the first available
  // branch (most-recently-committed, since the list is recency-sorted).
  // Runs once per branch-list arrival; if the user picks a valid
  // branch, `value` lands in the list and this no-ops.
  useEffect(() => {
    if (!branches || branches.length === 0) return;
    if (branches.some((b) => b.name === value)) return;
    const names = branches.map((b) => b.name);
    const best = names.includes("main")
      ? "main"
      : names.includes("master")
        ? "master"
        : names[0];
    onChange(best);
  }, [branches, value, onChange]);

  const handleSelect = (branch: string) => {
    onChange(branch);
    setOpen(false);
  };

  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) setFilterMode("all");
  };

  const allCount = branches?.length ?? 0;
  const worktreeCount = useMemo(() => {
    if (!branches) return 0;
    return branches.filter((b) => worktreeBranches.has(b.name)).length;
  }, [branches, worktreeBranches]);

  const visibleBranches = useMemo(() => {
    if (!branches) return [] as BranchDetail[];
    if (filterMode === "all") return branches;
    return branches.filter((b) => worktreeBranches.has(b.name));
  }, [branches, filterMode, worktreeBranches]);

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label="Derivative branch"
          className="inline-flex h-7 items-center gap-1.5 rounded-full border border-border bg-muted/40 px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <span className="text-[10px] opacity-60">from</span>
          <GitBranch className="h-3 w-3" />
          <span className="max-w-[200px] truncate font-mono">{value}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[340px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandInput placeholder="Search branches…" className="h-8" />
          <div className="mx-2 mt-1 mb-1 flex items-center gap-0.5 rounded-md bg-muted/40 p-0.5">
            <button
              type="button"
              className={cn(
                "flex-1 rounded-md px-2 py-1 text-xs transition-colors",
                filterMode === "all"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("all")}
            >
              All <span className="text-[10px] opacity-60">{allCount}</span>
            </button>
            <button
              type="button"
              className={cn(
                "flex-1 rounded-md px-2 py-1 text-xs transition-colors",
                filterMode === "worktrees"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("worktrees")}
            >
              Worktrees{" "}
              <span className="text-[10px] opacity-60">{worktreeCount}</span>
            </button>
          </div>
          <CommandList
            className="max-h-[320px] overflow-y-auto [scrollbar-width:thin]"
            onWheel={(e) => e.stopPropagation()}
          >
            <CommandEmpty>
              {loading
                ? "Loading…"
                : filterMode === "worktrees"
                  ? "No active worktrees"
                  : "No branches"}
            </CommandEmpty>
            {visibleBranches.length > 0 && (
              <CommandGroup>
                {visibleBranches.map((branch) => {
                  const hasWorktree = worktreeBranches.has(branch.name);
                  return (
                    <CommandItem
                      key={branch.name}
                      value={branch.name}
                      onSelect={() => handleSelect(branch.name)}
                      className="h-8 gap-2 px-2 text-xs"
                      data-checked={branch.name === value ? "true" : undefined}
                    >
                      <BranchKindIcon
                        hasWorktree={hasWorktree}
                        isLocal={branch.is_local}
                        isRemote={branch.is_remote}
                      />
                      <span className="min-w-0 flex-1 truncate font-mono">
                        {branch.name}
                      </span>
                      <span className="shrink-0 tabular-nums text-[11px] text-muted-foreground/60">
                        {formatRelativeTime(branch.last_commit_unix)}
                      </span>
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function BranchKindIcon({
  hasWorktree,
  isLocal,
  isRemote,
}: {
  hasWorktree: boolean;
  isLocal: boolean;
  isRemote: boolean;
}) {
  if (hasWorktree) {
    return <FolderGit className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (isRemote) {
    return <Globe className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (isLocal) {
    return <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  return <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />;
}
