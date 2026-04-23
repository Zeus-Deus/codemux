import { useEffect, useRef, useState } from "react";
import { ChevronDown, GitBranch, Loader2, Plus } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { toast } from "@/lib/toast";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import type { ChatDraft, DraftTarget } from "@/stores/chat-draft-store";
import {
  createWorktreeWorkspace,
  generateRandomBranchName,
} from "@/tauri/commands";

import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

import type { WorkspaceSnapshot } from "@/tauri/types";

// Module-scoped stable empty array — used as the fallback when
// `appState` is null. Returning a fresh `[]` literal from a Zustand
// selector triggers React's "getSnapshot should be cached" warning
// and (with `useProjectGroupedWorkspaces` consuming the array) loops.
const EMPTY_WORKSPACES: WorkspaceSnapshot[] = [];

interface WorktreePickerProps {
  mode: "draft" | "active";
  projectPath: string;

  /** Active mode — pane is on a real workspace. */
  currentWorkspaceId?: string;
  onSwitchWorkspace?: (workspaceId: string) => void;

  /** Draft mode — pane is a draft surface. */
  draftTarget?: ChatDraft["target"];
  onChangeDraftTarget?: (target: DraftTarget) => void;

  /** Base branch the new worktree forks from. Feeds the inline
   *  "+ New worktree…" submit. */
  derivativeBranch: string;

  /** Fires after `createWorktreeWorkspace` resolves. Parent decides
   *  whether to activate the new workspace, clear an active draft, etc. */
  onWorktreeCreated: (workspaceId: string) => void;

  disabled?: boolean;
}

function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() || path;
}

export function WorktreePicker({
  mode,
  projectPath,
  currentWorkspaceId,
  onSwitchWorkspace,
  draftTarget,
  onChangeDraftTarget,
  derivativeBranch,
  onWorktreeCreated,
  disabled,
}: WorktreePickerProps) {
  const [open, setOpen] = useState(false);

  const homeDir = useHomeDir();
  const workspaces = useAppStore(
    (s) => s.appState?.workspaces ?? EMPTY_WORKSPACES,
  );
  const groups = useProjectGroupedWorkspaces(workspaces, homeDir);
  const currentGroup = groups.find((g) => g.projectPath === projectPath);
  const worktrees = currentGroup?.workspaces ?? [];

  // Trigger pill label resolution.
  const triggerLabel = (() => {
    if (mode === "active") {
      const ws = worktrees.find((w) => w.workspace_id === currentWorkspaceId);
      return ws?.git_branch ?? (ws ? basename(ws.cwd) : basename(projectPath));
    }
    // Draft mode.
    if (!draftTarget) return basename(projectPath);
    if (draftTarget.kind === "project") {
      return basename(draftTarget.projectPath);
    }
    if (draftTarget.kind === "existing_workspace") {
      const ws = worktrees.find(
        (w) => w.workspace_id === draftTarget.workspaceId,
      );
      return ws?.git_branch ?? (ws ? basename(ws.cwd) : basename(projectPath));
    }
    return basename(projectPath);
  })();

  // Whether a given worktree row should render the "active" badge.
  const isCurrentWorkspace = (workspaceId: string): boolean => {
    if (mode === "active") return workspaceId === currentWorkspaceId;
    if (
      draftTarget?.kind === "existing_workspace" &&
      draftTarget.workspaceId === workspaceId
    ) {
      return true;
    }
    return false;
  };

  const handleRowSelect = (workspaceId: string) => {
    setOpen(false);
    if (mode === "active") {
      onSwitchWorkspace?.(workspaceId);
    } else {
      onChangeDraftTarget?.({ kind: "existing_workspace", workspaceId });
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none disabled:opacity-50"
        >
          <GitBranch className="h-3 w-3" />
          <span className="max-w-[200px] truncate font-mono">
            {triggerLabel}
          </span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[320px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandList className="max-h-[320px]">
            <CommandEmpty>No worktrees</CommandEmpty>
            {worktrees.length > 0 && (
              <CommandGroup>
                {worktrees.map((ws) => {
                  const label = ws.git_branch ?? basename(ws.cwd);
                  const isCurrent = isCurrentWorkspace(ws.workspace_id);
                  return (
                    <CommandItem
                      key={ws.workspace_id}
                      value={ws.workspace_id}
                      onSelect={() => handleRowSelect(ws.workspace_id)}
                      className="h-9 text-xs gap-2"
                    >
                      <GitBranch className="size-3.5 text-muted-foreground" />
                      <span className="flex-1 min-w-0 truncate font-mono">
                        {label}
                      </span>
                      {isCurrent && (
                        <Badge
                          variant="secondary"
                          className="text-[9px] h-4 px-1.5"
                        >
                          active
                        </Badge>
                      )}
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}
            {worktrees.length > 0 && <CommandSeparator />}
            <CommandGroup>
              <NewWorktreeRow
                projectPath={projectPath}
                derivativeBranch={derivativeBranch}
                onCreated={(wsId) => {
                  setOpen(false);
                  onWorktreeCreated(wsId);
                }}
              />
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

interface NewWorktreeRowProps {
  projectPath: string;
  derivativeBranch: string;
  onCreated: (workspaceId: string) => void;
}

/**
 * The "+ New worktree…" row. In its resting state it behaves like any
 * other picker row — click to activate. Once activated, the label
 * crossfades to an inline input that accepts a branch name; Enter
 * submits (empty → auto-generated name), Escape cancels.
 */
function NewWorktreeRow({
  projectPath,
  derivativeBranch,
  onCreated,
}: NewWorktreeRowProps) {
  const [isEditing, setIsEditing] = useState(false);
  const [name, setName] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
    }
  }, [isEditing]);

  const handleSubmit = async () => {
    if (submitting) return;
    setSubmitting(true);
    try {
      let branchName = name.trim();
      if (!branchName) {
        // Mirror NewWorkspaceDialog's "no prompt, no name" fallback —
        // reuse the backend's random-name generator so the naming
        // convention stays consistent.
        branchName = await generateRandomBranchName(projectPath);
      }
      const workspaceId = await createWorktreeWorkspace(
        projectPath,
        branchName,
        true,
        "single",
        derivativeBranch || null,
        null,
        null,
      );
      onCreated(workspaceId);
    } catch (err) {
      console.error("Failed to create worktree:", err);
      toast.error(`Failed to create worktree: ${err}`);
      setSubmitting(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    // Stop cmdk from observing arrow keys / Enter — the input owns
    // them while editing.
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      void handleSubmit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      setIsEditing(false);
      setName("");
    }
  };

  // The CommandItem's `onSelect` fires for both click and Enter-on-
  // highlight. In both cases we want to transform to edit mode.
  const handleSelect = () => {
    if (!isEditing) setIsEditing(true);
  };

  return (
    <CommandItem
      value="__new_worktree__"
      onSelect={handleSelect}
      // Preventing cmdk's default "clear selection on mouse move over
      // another row" behaviour would be nice here, but the edit-in-
      // place keeps selection the same row so it doesn't actually bite.
      className="h-9 text-xs gap-2 relative"
    >
      {submitting ? (
        <Loader2 className="size-3.5 text-muted-foreground shrink-0 animate-spin" />
      ) : (
        <Plus className="size-3.5 text-muted-foreground shrink-0" />
      )}
      {isEditing ? (
        <input
          ref={inputRef}
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={handleKeyDown}
          onClick={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
          onBlur={() => {
            // Cancel on blur when the user hasn't committed.
            if (!submitting && !name) {
              setIsEditing(false);
            }
          }}
          placeholder="branch name (leave empty for auto)"
          className="flex-1 bg-transparent outline-none text-xs font-mono placeholder:text-muted-foreground/50 animate-in fade-in-0 slide-in-from-left-1 duration-150"
          disabled={submitting}
          aria-label="New worktree branch name"
        />
      ) : (
        <span className="flex-1 animate-in fade-in-0 duration-150">
          New worktree…
        </span>
      )}
    </CommandItem>
  );
}
