import { useEffect, useState } from "react";
import { ChevronDown, GitBranch } from "lucide-react";

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
import { listBranches } from "@/tauri/commands";

import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

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
  const [branches, setBranches] = useState<string[] | null>(null);
  const [loading, setLoading] = useState(false);

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
    Promise.all([
      listBranches(projectPath, false).catch(() => [] as string[]),
      listBranches(projectPath, true).catch(() => [] as string[]),
    ])
      .then(([local, remote]) => {
        if (cancelled) return;
        const deduped = new Set<string>(local);
        for (const b of remote) deduped.add(b.replace(/^origin\//, ""));
        setBranches(Array.from(deduped).sort());
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
  // branch. Runs once per branch-list arrival; if the user picks a
  // valid branch, `value` lands in the list and this no-ops.
  useEffect(() => {
    if (!branches || branches.length === 0) return;
    if (branches.includes(value)) return;
    const best = branches.includes("main")
      ? "main"
      : branches.includes("master")
        ? "master"
        : branches[0];
    onChange(best);
  }, [branches, value, onChange]);

  const handleSelect = (branch: string) => {
    onChange(branch);
    setOpen(false);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          aria-label="Derivative branch"
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <span className="text-[10px] opacity-60">from</span>
          <GitBranch className="h-3 w-3" />
          <span className="max-w-[200px] truncate font-mono">{value}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[280px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandInput placeholder="Search branches…" />
          <CommandList className="max-h-[320px]">
            <CommandEmpty>
              {loading ? "Loading…" : "No branches"}
            </CommandEmpty>
            {branches && branches.length > 0 && (
              <CommandGroup>
                {branches.map((branch) => (
                  <CommandItem
                    key={branch}
                    value={branch}
                    onSelect={() => handleSelect(branch)}
                    className="h-8 text-xs gap-2"
                    data-checked={branch === value ? "true" : undefined}
                  >
                    <GitBranch className="size-3.5 text-muted-foreground" />
                    <span className="flex-1 min-w-0 truncate font-mono">
                      {branch}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
