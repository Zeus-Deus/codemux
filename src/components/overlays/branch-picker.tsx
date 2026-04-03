import { useState, useMemo } from "react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Badge } from "@/components/ui/badge";
import {
  GitBranch,
  Globe,
  ArrowUpRight,
  FolderGit,
  ChevronDown,
  CornerDownRight,
  Loader2,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { BranchDetail, WorktreeInfo } from "@/tauri/types";

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

type FilterMode = "all" | "worktrees";

// Stable defaults for optional Map/Set props (avoids new allocations per render)
const EMPTY_BRANCH_MAP = new Map<string, string>();
const EMPTY_PR_SET = new Set<string>();

interface BranchPickerProps {
  baseBranch: string;
  branches: BranchDetail[];
  loading: boolean;
  onSelectBase: (branch: string) => void;
  /** When workspace-aware callbacks are omitted, the picker runs in base-only selection mode */
  worktrees?: WorktreeInfo[];
  branchWorkspaceMap?: Map<string, string>;
  prBranches?: Set<string>;
  currentBranch?: string | null;
  onOpenWorkspace?: (workspaceId: string) => void;
  onImportWorktree?: (path: string, branch: string) => void;
  onCreateOnCurrent?: () => void;
  onOpenExisting?: (branch: string) => void;
  isOpenMode?: boolean;
}

export function BranchPicker({
  baseBranch,
  branches,
  loading,
  onSelectBase,
  worktrees = [],
  branchWorkspaceMap = EMPTY_BRANCH_MAP,
  prBranches = EMPTY_PR_SET,
  currentBranch = null,
  onOpenWorkspace,
  onImportWorktree,
  onCreateOnCurrent,
  onOpenExisting,
  isOpenMode,
}: BranchPickerProps) {
  const [open, setOpen] = useState(false);
  const [filterMode, setFilterMode] = useState<FilterMode>("all");
  const [search, setSearch] = useState("");

  // Base-only mode: no workspace context, simplified UI with single "Select" action
  const baseOnly = !onOpenWorkspace;

  // Reset state when popover closes
  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) {
      setFilterMode("all");
      setSearch("");
    }
  };

  // Build a set of branch names that have a worktree on disk
  const worktreeBranches = useMemo(() => {
    const set = new Set<string>();
    for (const wt of worktrees) {
      if (wt.branch) set.add(wt.branch);
    }
    return set;
  }, [worktrees]);

  // Find worktree for a given branch name
  const findWorktree = (name: string): WorktreeInfo | undefined =>
    worktrees.find((wt) => wt.branch === name);

  // Branches visible in current tab
  const tabBranches = useMemo(() => {
    if (filterMode === "all") return branches;
    return branches.filter(
      (b) => worktreeBranches.has(b.name) || branchWorkspaceMap.has(b.name),
    );
  }, [branches, filterMode, worktreeBranches, branchWorkspaceMap]);

  // Counts for tab badges
  const allCount = branches.length;
  const worktreeCount = useMemo(
    () =>
      branches.filter(
        (b) => worktreeBranches.has(b.name) || branchWorkspaceMap.has(b.name),
      ).length,
    [branches, worktreeBranches, branchWorkspaceMap],
  );

  // Handle primary action on a branch row (Enter key or click)
  const handlePrimaryAction = (branch: BranchDetail) => {
    if (baseOnly) {
      onSelectBase(branch.name);
      setOpen(false);
      return;
    }

    const wsId = branchWorkspaceMap.get(branch.name);
    if (wsId) {
      // Workspace is open — switch to it
      onOpenWorkspace?.(wsId);
      setOpen(false);
      return;
    }

    const wt = findWorktree(branch.name);
    if (wt) {
      // Worktree exists but no workspace — import it
      onImportWorktree?.(wt.path, branch.name);
      setOpen(false);
      return;
    }

    if (branch.name === currentBranch) {
      // Current branch — create workspace on existing checkout
      onCreateOnCurrent?.();
      setOpen(false);
      return;
    }

    // Default branch (main/master) — fork is the primary action
    const isDefault = branch.name === "main" || branch.name === "master";
    if (isDefault) {
      onSelectBase(branch.name);
      setOpen(false);
      return;
    }

    // Other branches — open existing is the primary action
    onOpenExisting?.(branch.name);
    setOpen(false);
  };

  // Handle secondary action (Ctrl+Enter or explicit button)
  const handleSecondaryAction = (
    e: React.MouseEvent | React.KeyboardEvent,
    branch: BranchDetail,
  ) => {
    e.stopPropagation();
    const isDefault = branch.name === "main" || branch.name === "master";
    const hasWorkspace = branchWorkspaceMap.has(branch.name);
    const hasWt = !!findWorktree(branch.name);
    // "Open" is only valid when the branch has no worktree yet
    if (isDefault && !hasWorkspace && !hasWt) {
      onOpenExisting?.(branch.name);
    } else {
      onSelectBase(branch.name);
    }
    setOpen(false);
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none"
        >
          {isOpenMode ? <CornerDownRight className="h-3 w-3" /> : <GitBranch className="h-3 w-3" />}
          <span className="max-w-[120px] truncate">{baseBranch}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-[380px] p-0" align="start">
        <Command
          shouldFilter={false}
          onKeyDown={baseOnly ? undefined : (e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              const active = e.currentTarget.querySelector<HTMLElement>('[data-selected="true"]');
              const name = active?.getAttribute("data-value");
              if (name) {
                const branch = branches.find((b) => b.name === name);
                if (branch) handleSecondaryAction(e, branch);
              }
            }
          }}
        >
          <CommandInput
            placeholder="Search branches..."
            className="h-8"
            value={search}
            onValueChange={setSearch}
          />

          {/* Tab bar — hidden in base-only mode */}
          {!baseOnly && <div className="flex items-center gap-0.5 mx-2 mt-1 mb-1 rounded-md bg-muted/40 p-0.5">
            <button
              type="button"
              className={cn(
                "flex-1 px-2 py-1 text-xs rounded-md transition-colors",
                filterMode === "all"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("all")}
            >
              All{" "}
              <span className="text-[10px] opacity-60">{allCount}</span>
            </button>
            <button
              type="button"
              className={cn(
                "flex-1 px-2 py-1 text-xs rounded-md transition-colors",
                filterMode === "worktrees"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("worktrees")}
            >
              Worktrees{" "}
              <span className="text-[10px] opacity-60">{worktreeCount}</span>
            </button>
          </div>}

          <CommandList
            className="max-h-[340px] overflow-y-auto [scrollbar-width:thin]"
            onWheel={(e) => e.stopPropagation()}
          >
            {loading ? (
              <div className="flex items-center justify-center py-8 text-xs text-muted-foreground">
                <Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />
                Loading branches...
              </div>
            ) : (
              <>
                <CommandEmpty>
                  {filterMode === "worktrees"
                    ? "No active worktrees"
                    : "No branches match"}
                </CommandEmpty>
                <CommandGroup>
                  {tabBranches
                    .filter(
                      (b) =>
                        !search ||
                        b.name
                          .toLowerCase()
                          .includes(search.toLowerCase()),
                    )
                    .map((branch) => {
                      const wsId = branchWorkspaceMap.get(branch.name);
                      const wt = findWorktree(branch.name);
                      const hasOpenWorkspace = !!wsId;
                      const hasWorktree = !!wt;
                      const hasPr = prBranches.has(branch.name);
                      const isDefault =
                        branch.name === "main" ||
                        branch.name === "master";

                      // Determine action labels based on branch state
                      const hasAnyWorktree = hasOpenWorkspace || hasWorktree;
                      const primaryLabel = baseOnly ? "Select"
                        : hasOpenWorkspace ? "Focus"
                        : (isDefault && !hasAnyWorktree) ? "Fork"
                        : "Open";
                      const secondaryLabel = (!hasAnyWorktree && isDefault) ? "Open" : "Fork";

                      // In base-only mode, all rows use hover-reveal; in full mode, default branch always shows actions
                      const alwaysShowActions = !baseOnly && isDefault;

                      return (
                        <CommandItem
                          key={branch.name}
                          value={branch.name}
                          onSelect={() => handlePrimaryAction(branch)}
                          className="h-9 text-xs gap-2 px-2 group/row"
                        >
                          {/* Icon */}
                          <BranchIcon
                            hasWorkspace={hasOpenWorkspace}
                            hasWorktree={hasWorktree}
                            isLocal={branch.is_local}
                          />

                          {/* Name */}
                          <span className="flex-1 min-w-0 truncate font-mono">
                            {branch.name}
                          </span>

                          {/* Badges */}
                          {isDefault && (
                            <Badge
                              variant="secondary"
                              className="text-[9px] px-1 py-0 shrink-0"
                            >
                              default
                            </Badge>
                          )}
                          {hasPr && (
                            <Badge
                              variant="secondary"
                              className="text-[9px] px-1 py-0 shrink-0 bg-purple-500/15 text-purple-400 border-purple-500/20"
                            >
                              PR
                            </Badge>
                          )}
                          {hasOpenWorkspace && (
                            <Badge
                              variant="secondary"
                              className="text-[9px] px-1 py-0 shrink-0 bg-success/10 text-success border-success/20"
                            >
                              active
                            </Badge>
                          )}

                          {/* Timestamp — visible at rest, hidden on hover to make room for actions */}
                          {!alwaysShowActions && (
                            <span className="text-[11px] text-muted-foreground/60 tabular-nums shrink-0 group-hover/row:hidden">
                              {formatRelativeTime(branch.last_commit_unix)}
                            </span>
                          )}

                          {/* Action buttons */}
                          <span className={cn(
                            "items-center gap-1 shrink-0",
                            alwaysShowActions ? "flex" : "hidden group-hover/row:flex",
                          )}>
                            <button
                              type="button"
                              className="inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium bg-foreground text-background hover:bg-foreground/90 transition-colors"
                              onClick={(e) => {
                                e.stopPropagation();
                                handlePrimaryAction(branch);
                              }}
                            >
                              {primaryLabel}
                              <kbd className="opacity-50">↵</kbd>
                            </button>
                            {!baseOnly && <button
                              type="button"
                              className="inline-flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium bg-muted text-muted-foreground hover:bg-muted/80 transition-colors"
                              onClick={(e) => handleSecondaryAction(e, branch)}
                            >
                              {secondaryLabel}
                              <kbd className="opacity-50">⌃↵</kbd>
                            </button>}
                          </span>
                        </CommandItem>
                      );
                    })}
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function BranchIcon({
  hasWorkspace,
  hasWorktree,
  isLocal,
}: {
  hasWorkspace: boolean;
  hasWorktree: boolean;
  isLocal: boolean;
}) {
  if (hasWorkspace) {
    return <ArrowUpRight className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (hasWorktree) {
    return <FolderGit className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (isLocal) {
    return <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  return <Globe className="size-3.5 shrink-0 text-muted-foreground" />;
}
