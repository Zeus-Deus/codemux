import { useEffect, useMemo, useRef, useState } from "react";
import { CornerDownLeft, X } from "lucide-react";

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { basename } from "@/lib/path";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { renameWorkspace } from "@/tauri/commands";

export const WORKSPACE_NAME_MAX_LENGTH = 48;

type RenameStatus =
  | { kind: "empty" | "long" | "taken"; message: string }
  | { kind: "idle" | "ready"; message: string };

export function getWorkspaceRenameStatus({
  name,
  originalName,
  branch,
  takenNames,
}: {
  name: string;
  originalName: string;
  branch: string | null;
  takenNames: readonly string[];
}): RenameStatus {
  const value = name.trim();
  if (!value) return { kind: "empty", message: "Name can’t be empty" };
  if (value.length > WORKSPACE_NAME_MAX_LENGTH) {
    return {
      kind: "long",
      message: `Keep it under ${WORKSPACE_NAME_MAX_LENGTH} characters`,
    };
  }
  // Case-insensitive: two sibling rows called "Docs" and "docs" are the same
  // label to anyone scanning the sidebar. Normalization lives here so callers
  // can hand over raw titles.
  const folded = value.toLowerCase();
  if (takenNames.some((taken) => taken.trim().toLowerCase() === folded)) {
    return {
      kind: "taken",
      message: "Another workspace already uses this name",
    };
  }

  const branchMessage = branch
    ? `Branch stays ${branch}`
    : "Only the workspace label changes";
  if (value === originalName) {
    return { kind: "idle", message: branchMessage };
  }
  return {
    kind: "ready",
    message: branch ? `${branchMessage} — only the label changes` : branchMessage,
  };
}

/**
 * Mounted at the app root, so it must cost nothing while closed: the outer
 * component watches one id and never touches `appState` until a rename is
 * actually requested. The `key` also guarantees the body starts from scratch
 * for each target instead of carrying the previous name over.
 */
export function RenameWorkspaceDialog() {
  const workspaceId = useUIStore((s) => s.renameWorkspaceId);
  if (!workspaceId) return null;
  return <RenameWorkspaceDialogBody key={workspaceId} workspaceId={workspaceId} />;
}

function RenameWorkspaceDialogBody({ workspaceId }: { workspaceId: string }) {
  const closeRenameWorkspace = useUIStore((s) => s.closeRenameWorkspace);
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const workspace = workspaces?.find(
    (candidate) => candidate.workspace_id === workspaceId,
  );
  const [name, setName] = useState(() => workspace?.title ?? "");
  const [touched, setTouched] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const fieldRef = useRef<HTMLDivElement>(null);

  // The workspace can be archived or closed from another surface while the
  // dialog is up. Drop the request too, or the id lingers as a phantom rung on
  // the Escape ladder that swallows a press and shows nothing.
  useEffect(() => {
    if (!workspace) closeRenameWorkspace();
  }, [workspace, closeRenameWorkspace]);

  // Only siblings in the same project can collide: identical names across two
  // unrelated projects read fine, each under its own project header.
  const projectKey = workspace ? (workspace.project_root ?? workspace.cwd) : null;
  const takenNames = useMemo(
    () =>
      workspaces
        ?.filter(
          (candidate) =>
            candidate.workspace_id !== workspaceId &&
            (candidate.project_root ?? candidate.cwd) === projectKey,
        )
        .map((candidate) => candidate.title) ?? [],
    [workspaces, workspaceId, projectKey],
  );

  if (!workspace) return null;

  const status = getWorkspaceRenameStatus({
    name,
    originalName: workspace.title,
    branch: workspace.git_branch,
    takenNames,
  });
  const isError =
    status.kind === "empty" || status.kind === "long" || status.kind === "taken";
  const showError = touched && isError;
  const trimmedName = name.trim();
  const isDirty = status.kind === "ready";
  const projectPath = workspace.project_root ?? workspace.cwd;
  const projectName = basename(projectPath) || projectPath;
  const contextLabel = workspace.git_branch
    ? `${projectName} · ${workspace.git_branch}`
    : projectName;

  const rejectInvalid = () => {
    setTouched(true);
    const field = fieldRef.current;
    if (field) {
      field.style.animation = "none";
      void field.offsetWidth;
      field.style.animation = "rename-field-shake .22s ease";
    }
    window.requestAnimationFrame(() => inputRef.current?.focus());
  };

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (isError) {
      rejectInvalid();
      return;
    }
    if (!isDirty || submitting) return;

    setSubmitting(true);
    try {
      await renameWorkspace(workspace.workspace_id, trimmedName);
      closeRenameWorkspace();
      toast.success(`Renamed to ${trimmedName}`);
    } catch (error) {
      setSubmitting(false);
      toast.error("Rename failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) closeRenameWorkspace();
      }}
    >
      <DialogContent
        showCloseButton={false}
        overlayClassName="!bg-black/60 supports-backdrop-filter:backdrop-blur-[3px]"
        // Escape belongs to the app's close ladder, which knows this dialog
        // sits above Settings and closes exactly one layer per press. Letting
        // the dialog dismiss itself too would close both at once, since the
        // ladder runs after and finds the rung already gone.
        onEscapeKeyDown={(event) => event.preventDefault()}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          inputRef.current?.focus();
          inputRef.current?.select();
        }}
        className={cn(
          "top-[46%] flex w-[436px] max-w-[calc(100vw-48px)] flex-col gap-0 overflow-hidden rounded-[14px] border border-border bg-popover p-0 text-popover-foreground ring-0",
          "shadow-[0_32px_90px_oklch(0_0_0/.62),inset_0_2px_0_oklch(1_0_0/.04)]",
          "sm:max-w-[436px]",
        )}
      >
        <div className="flex items-center gap-2.5 border-b border-border/70 py-[13px] pr-3.5 pl-4">
          <span
            aria-hidden
            className="flex size-6 shrink-0 items-center justify-center rounded-[7px] bg-accent-ember/20 font-mono text-[10.5px] font-medium text-accent-ember"
          >
            W
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <DialogTitle className="text-[13.5px] leading-tight font-semibold tracking-[-0.012em]">
              Rename workspace
            </DialogTitle>
            <DialogDescription
              className="truncate font-mono text-[9.5px] leading-tight text-muted-foreground/70"
            >
              {contextLabel}
            </DialogDescription>
          </span>
          <DialogClose asChild>
            <button
              type="button"
              aria-label="Close rename workspace dialog"
              className="flex size-6 shrink-0 items-center justify-center rounded-[7px] text-muted-foreground/65 transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <X className="size-3" strokeWidth={1.6} />
            </button>
          </DialogClose>
        </div>

        <form onSubmit={(event) => void handleSubmit(event)}>
          <div className="flex flex-col gap-[9px] px-4 pt-4 pb-3.5">
            <label
              htmlFor="rename-workspace-name"
              className="font-mono text-[9px] leading-none tracking-[0.16em] text-muted-foreground/60 uppercase"
            >
              Workspace name
            </label>
            <div
              ref={fieldRef}
              className={cn(
                "flex h-10 items-center gap-[9px] rounded-[9px] border bg-background/70 px-[11px] transition-[border-color,box-shadow] duration-100",
                showError
                  ? "border-status-attention/60 shadow-[0_0_0_3px_color-mix(in_oklch,var(--status-attention)_14%,transparent)]"
                  : "border-accent-ember/55 shadow-[0_0_0_3px_color-mix(in_oklch,var(--accent-ember)_13%,transparent)]",
              )}
            >
              <input
                ref={inputRef}
                id="rename-workspace-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                  setTouched(true);
                }}
                aria-invalid={showError}
                aria-describedby="rename-workspace-hint"
                autoComplete="off"
                spellCheck={false}
                placeholder="add-gpt5-benchmark-runs"
                className="h-6 min-w-0 flex-1 border-0 bg-transparent p-0 text-sm tracking-[-0.005em] text-foreground outline-none placeholder:text-muted-foreground/60 selection:bg-accent-ember/40"
              />
              {name.length > 0 && (
                <button
                  type="button"
                  aria-label="Clear workspace name"
                  onClick={() => {
                    setName("");
                    setTouched(true);
                    inputRef.current?.focus();
                  }}
                  className="flex size-[18px] shrink-0 items-center justify-center rounded-[5px] text-muted-foreground/65 transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  <X className="size-2.5" strokeWidth={1.8} />
                </button>
              )}
              <span
                className={cn(
                  "shrink-0 font-mono text-[9.5px] tabular-nums",
                  trimmedName.length > WORKSPACE_NAME_MAX_LENGTH
                    ? "text-status-attention"
                    : "text-muted-foreground/55",
                )}
              >
                {trimmedName.length}/{WORKSPACE_NAME_MAX_LENGTH}
              </span>
            </div>
            <p
              id="rename-workspace-hint"
              className={cn(
                "min-h-[15px] font-mono text-[10px] leading-[15px] text-pretty",
                showError
                  ? "text-status-attention"
                  : status.kind === "ready"
                    ? "text-muted-foreground/75"
                    : "text-muted-foreground/55",
              )}
            >
              {status.message}
            </p>
          </div>

          <div className="flex items-center gap-2 border-t border-border/70 bg-muted/30 px-3 py-[11px]">
            <span className="flex min-w-0 flex-1 items-center gap-1.5 font-mono text-[9.5px] text-muted-foreground/55">
              <kbd className="rounded-[5px] border border-border/70 px-[5px] py-0.5 font-mono">
                esc
              </kbd>
              <span>cancel</span>
            </span>
            <DialogClose asChild>
              <button
                type="button"
                className="flex h-[31px] shrink-0 items-center rounded-lg px-[13px] text-[12.5px] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                Cancel
              </button>
            </DialogClose>
            <button
              type="submit"
              disabled={submitting}
              aria-disabled={!isDirty || isError}
              className={cn(
                "flex h-[31px] shrink-0 items-center gap-2 rounded-lg pr-3 pl-[13px] text-[12.5px] font-semibold tracking-[-0.005em] transition-[background-color,opacity,filter] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                isDirty && !isError
                  ? "cursor-pointer bg-accent-ember text-[oklch(0.16_0.02_47)] hover:brightness-105"
                  : "cursor-not-allowed bg-accent text-muted-foreground",
                isError && "opacity-50",
                submitting && "opacity-60",
              )}
            >
              <span>{submitting ? "Renaming…" : "Rename"}</span>
              <CornerDownLeft className="size-[11px] opacity-75" strokeWidth={1.8} />
            </button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
