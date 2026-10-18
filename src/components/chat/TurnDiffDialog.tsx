import { FileText } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useUIStore } from "@/stores/ui-store";
import { agentChatGetTurnDiff } from "@/tauri/commands";
import type { AgentChatTurnDiff, TurnDiffFile } from "@/tauri/types";

import { ChatFileLinkContext } from "./chat-file-link-context";
import { DiffView } from "./DiffView";

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "loaded"; diff: AgentChatTurnDiff | null };

const OMITTED_REASON: Record<NonNullable<TurnDiffFile["patch_omitted"]>, string> = {
  binary: "Binary file",
  too_large: "Too large to show",
  budget: "Not shown: this turn's diff is too large",
};

function fileLabel(file: TurnDiffFile): string {
  return file.old_path ? `${file.old_path} → ${file.path}` : file.path;
}

/** A file that changed but has no patch to show: its path, line counts and
 *  why the patch is missing, in the same frame as a `DiffView`. */
function OmittedFile({ file }: { file: TurnDiffFile }) {
  const reason = file.patch_omitted ? OMITTED_REASON[file.patch_omitted] : "No textual changes";
  return (
    <div
      className="flex items-center justify-between gap-3 rounded-lg border border-hairline bg-surface-1 px-[13px] py-[9px]"
      data-testid="turn-diff-omitted"
    >
      <span className="flex min-w-0 items-center gap-2 font-mono text-body-sm text-muted-foreground">
        <FileText className="size-3 shrink-0 text-muted-foreground/70" aria-hidden />
        <span className="truncate" title={fileLabel(file)}>
          {fileLabel(file)}
        </span>
      </span>
      <span className="flex shrink-0 items-center gap-2.5 text-label text-muted-foreground">
        <span>{reason}</span>
        {file.additions + file.deletions > 0 ? (
          <span className="font-mono">
            <span className="text-status-open">+{file.additions}</span>{" "}
            <span className="text-status-attention">−{file.deletions}</span>
          </span>
        ) : null}
      </span>
    </div>
  );
}

function Notice({ children }: { children: ReactNode }) {
  return (
    <p className="py-6 text-center text-body-sm text-muted-foreground" data-testid="turn-diff-notice">
      {children}
    </p>
  );
}

function Summary({ diff }: { diff: AgentChatTurnDiff }) {
  const count = diff.files_changed;
  return (
    <>
      {count} {count === 1 ? "file" : "files"} changed ·{" "}
      <span className="font-mono">
        <span className="text-status-open">+{diff.additions}</span>{" "}
        <span className="text-status-attention">−{diff.deletions}</span>
      </span>
    </>
  );
}

/**
 * The real git diff one agent turn made, between worktree snapshots taken
 * right before and right after it: one `DiffView` per file, with binary or
 * oversized files listed by name. Turns that were not captured (not a git
 * repository, or from before capture) point to the Changes pane instead.
 */
export function TurnDiffDialog({
  open,
  onOpenChange,
  threadId,
  turnId,
  workspaceId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  threadId: string;
  turnId: string;
  workspaceId?: string | null;
}) {
  const [load, setLoad] = useState<Load>({ state: "loading" });

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoad({ state: "loading" });
    agentChatGetTurnDiff(threadId, turnId).then(
      (diff) => {
        if (!cancelled) setLoad({ state: "loaded", diff });
      },
      (error: unknown) => {
        if (!cancelled) setLoad({ state: "error", message: String(error) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [open, threadId, turnId]);

  const diff = load.state === "loaded" ? load.diff : null;
  const openChangesPane = workspaceId
    ? () => {
        useUIStore.getState().setRightPanelTab(workspaceId, "changes");
        onOpenChange(false);
      }
    : null;

  let body: ReactNode;
  if (load.state === "loading") {
    body = <Notice>Loading this turn&apos;s changes…</Notice>;
  } else if (load.state === "error") {
    body = <Notice>Couldn&apos;t load this turn&apos;s changes: {load.message}</Notice>;
  } else if (!diff) {
    body = (
      <Notice>
        This turn&apos;s changes weren&apos;t captured. Codemux records a diff for turns
        that run in a git repository.
      </Notice>
    );
  } else if (diff.status === "in_progress") {
    body = <Notice>This turn is still running. Its changes show here once it finishes.</Notice>;
  } else if (diff.files.length === 0) {
    body = <Notice>This turn didn&apos;t change any files.</Notice>;
  } else {
    body = (
      <ChatFileLinkContext.Provider value={{ workspaceId, cwd: diff.repo_root }}>
        <div className="flex flex-col gap-3" data-testid="turn-diff-files">
          {diff.files.map((file) =>
            file.patch !== null ? (
              <DiffView
                key={file.path}
                filename={file.path}
                patch={file.patch}
                copyText={file.patch}
              />
            ) : (
              <OmittedFile key={file.path} file={file} />
            ),
          )}
          {diff.truncated ? (
            <Notice>
              Some changes are not shown here. Open the Changes pane or use git to see
              everything.
            </Notice>
          ) : null}
        </div>
      </ChatFileLinkContext.Provider>
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="flex max-h-[calc(100dvh-4rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-3xl"
        data-testid="turn-diff-dialog"
      >
        <DialogHeader className="border-b border-hairline px-4 py-3">
          <DialogTitle>Changes in this turn</DialogTitle>
          <DialogDescription>
            {diff && diff.status === "ready" ? (
              <Summary diff={diff} />
            ) : (
              "What this turn changed in the workspace, as a git diff."
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto px-4 py-3">{body}</div>
        {openChangesPane ? (
          <DialogFooter className="mx-0 mb-0 border-hairline px-4 py-3">
            <Button variant="ghost" size="sm" onClick={openChangesPane}>
              Open Changes pane
            </Button>
          </DialogFooter>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
