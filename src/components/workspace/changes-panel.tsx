import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "@/components/ui/tooltip";
import {
  Loader2,
  Sparkles,
  GitBranch,
  ArrowUp,
  ArrowDown,
  ArrowUpDown,
  Download,
  Trash2,
  Plus,
  Minus,
  Check,
  ChevronDown,
  X,
  AlertTriangle,
  GitMerge,
  Pencil,
  Undo2,
  Archive,
  ArchiveRestore,
  Send,
} from "lucide-react";
import {
  VscDiffAdded,
  VscDiffModified,
  VscDiffRemoved,
  VscDiffRenamed,
} from "react-icons/vsc";
import {
  getGitStatus,
  getGitBranchInfo,
  gitStageFiles,
  gitUnstageFiles,
  gitCommitChanges,
  gitPushChanges,
  gitPullChanges,
  gitFetchChanges,
  gitDiscardFile,
  gitAmendCommit,
  gitUndoLastCommit,
  gitStashPush,
  gitStashPop,
  getMergeState,
  abortMerge,
  continueMerge,
  createTab,
  activateTab,
  checkClaudeAvailable,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { keepIfUnchanged } from "@/lib/poll-equality";
import { useQueryClient } from "@tanstack/react-query";
import { useDiffStore } from "@/stores/diff-store";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useAiCommitStore } from "@/stores/ai-commit-store";
import { showNoGitState, useInitializeGit } from "@/hooks/use-initialize-git";
import { cn } from "@/lib/utils";
import { utilitySelectionFromStores } from "@/lib/utility-agent";
import type {
  WorkspaceSnapshot,
  GitFileStatus,
  GitBranchInfo,
  MergeState,
} from "@/tauri/types";

/** Which file sections the panel lists. Driven by the deck's pane-bar
 *  filter; `"all"` is the historic behavior. */
export type ChangesSectionFilter = "all" | "staged" | "unstaged" | "conflicts";

interface Props {
  workspace: WorkspaceSnapshot;
  /** Bumped by the deck's pane-bar Refresh — the panel's own header (and
   *  the refresh button in it) moved into the shared pane bar. */
  refreshKey?: number;
  sectionFilter?: ChangesSectionFilter;
  /** Where a file row's diff opens. Defaults to a main-area diff tab; the
   *  deck routes it to its own Diff pane so the click stays in the panel. */
  onOpenDiff?: (filePath: string, staged: boolean) => void;
}

// ── File-status icon mapping ──
//
// Each git status carries a color through the file row; everything else
// stays muted so the eye lands on what changed, not on the chrome.
const STATUS_META: Record<string, { icon: React.ReactNode; color: string; label: string }> = {
  added:      { icon: <VscDiffAdded className="size-3" />,    color: "text-success",  label: "Added" },
  modified:   { icon: <VscDiffModified className="size-3" />, color: "text-warning",  label: "Modified" },
  removed:    { icon: <VscDiffRemoved className="size-3" />,  color: "text-danger",   label: "Removed" },
  renamed:    { icon: <VscDiffRenamed className="size-3" />,  color: "text-info",     label: "Renamed" },
  untracked:  { icon: <VscDiffAdded className="size-3" />,    color: "text-success/70", label: "Untracked" },
  conflicted: { icon: <AlertTriangle className="size-3" />,   color: "text-danger",   label: "Conflict" },
};

function fileName(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i >= 0 ? path.slice(i + 1) : path;
}

// ── FileRow ──

function FileRow({
  file,
  staged,
  cwd,
  onRefresh,
  onOpenDiff,
}: {
  file: GitFileStatus;
  staged: boolean;
  cwd: string;
  onRefresh: () => void;
  onOpenDiff: (filePath: string, staged: boolean) => void;
}) {
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const meta = STATUS_META[file.status] ?? STATUS_META.modified;
  const name = fileName(file.path);
  const dir = file.path.length > name.length ? file.path.slice(0, -name.length - 1) : "";

  const handleStageToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      if (staged) await gitUnstageFiles(cwd, [file.path]);
      else await gitStageFiles(cwd, [file.path]);
      onRefresh();
    } catch (err) {
      toast.error(String(err));
    }
  };

  // Two-tap discard so a stray hover-click can't blow away uncommitted
  // work. The 3s timeout reverts the row to its idle state if the user
  // doesn't follow through.
  const handleDiscard = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirmDiscard) {
      setConfirmDiscard(true);
      setTimeout(() => setConfirmDiscard(false), 3000);
      return;
    }
    try {
      await gitDiscardFile(cwd, file.path);
      onRefresh();
    } catch (err) {
      toast.error(String(err));
    }
    setConfirmDiscard(false);
  };

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          role="button"
          tabIndex={0}
          onClick={() => onOpenDiff(file.path, staged)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onOpenDiff(file.path, staged);
            }
          }}
          className="group/file flex items-center gap-1.5 px-2.5 h-6 cursor-default rounded-sm hover:bg-muted/40 transition-colors"
        >
          <span className={cn("shrink-0 flex items-center justify-center w-3", meta.color)}>
            {meta.icon}
          </span>
          <span className="truncate text-xs text-foreground min-w-0 flex-1">
            {name}
            {dir && (
              <span className="ml-1 text-[10px] text-muted-foreground/40">{dir}</span>
            )}
          </span>
          {(file.additions > 0 || file.deletions > 0) && (
            <span className="shrink-0 flex items-center gap-1 text-[10px] tabular-nums text-muted-foreground/60 group-hover/file:opacity-0 transition-opacity">
              {file.additions > 0 && <span className="text-success">+{file.additions}</span>}
              {file.deletions > 0 && <span className="text-danger">{file.deletions}</span>}
            </span>
          )}
          <span className="shrink-0 hidden group-hover/file:flex items-center gap-0.5 ml-auto absolute right-1">
            <Button
              variant="ghost"
              size="icon-xs"
              className={cn("size-5", confirmDiscard ? "text-danger" : "text-muted-foreground hover:text-foreground")}
              onClick={handleDiscard}
              title={confirmDiscard ? "Click again to discard" : "Discard changes"}
            >
              <Trash2 className="size-3" />
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              className="size-5 text-muted-foreground hover:text-foreground"
              onClick={handleStageToggle}
              title={staged ? "Unstage" : "Stage"}
            >
              {staged ? <Minus className="size-3" /> : <Plus className="size-3" />}
            </Button>
          </span>
        </div>
      </TooltipTrigger>
      <TooltipContent side="left" className="text-xs">
        {file.path}
      </TooltipContent>
    </Tooltip>
  );
}

// ── FileSection (Staged / Changed grouping) ──

function FileSection({
  label,
  files,
  staged,
  cwd,
  onRefresh,
  onOpenDiff,
}: {
  label: string;
  files: GitFileStatus[];
  staged: boolean;
  cwd: string;
  onRefresh: () => void;
  onOpenDiff: (filePath: string, staged: boolean) => void;
}) {
  if (files.length === 0) return null;
  return (
    <div className="mb-2">
      <div className="flex items-center px-2.5 h-5 text-[10px] font-medium tracking-wider uppercase text-muted-foreground/60">
        <span>{label}</span>
        <span className="ml-1.5 tabular-nums text-muted-foreground/40">{files.length}</span>
      </div>
      <div className="flex flex-col">
        {files.map((file) => (
          <FileRow
            key={`${staged ? "s" : "u"}-${file.path}`}
            file={file}
            staged={staged}
            cwd={cwd}
            onRefresh={onRefresh}
            onOpenDiff={onOpenDiff}
          />
        ))}
      </div>
    </div>
  );
}

// ── BranchPill ──

function BranchPill({ info }: { info: GitBranchInfo | null }) {
  if (!info?.branch) return null;
  const ahead = info.ahead ?? 0;
  const behind = info.behind ?? 0;
  return (
    <div className="flex items-center gap-1.5 px-2.5 h-7 text-[11px] text-muted-foreground/80 border-b border-border/40">
      <GitBranch className="size-3 shrink-0" />
      <span className="truncate font-mono text-foreground/90">{info.branch}</span>
      {(ahead > 0 || behind > 0) && (
        <span className="ml-auto flex items-center gap-1.5 tabular-nums text-[10px]">
          {behind > 0 && (
            <span className="flex items-center gap-0.5 text-warning">
              <ArrowDown className="size-2.5" />
              {behind}
            </span>
          )}
          {ahead > 0 && (
            <span className="flex items-center gap-0.5 text-info">
              <ArrowUp className="size-2.5" />
              {ahead}
            </span>
          )}
        </span>
      )}
      {!info.has_upstream && (
        <span className="ml-auto text-[10px] italic text-muted-foreground/60">no remote</span>
      )}
    </div>
  );
}

// ── ChangesPanel ──

export function ChangesPanel({
  workspace,
  refreshKey = 0,
  sectionFilter = "all",
  onOpenDiff: onOpenDiffOverride,
}: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const queryClient = useQueryClient();

  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [branchInfo, setBranchInfo] = useState<GitBranchInfo | null>(null);
  const [mergeState, setMergeState] = useState<MergeState | null>(null);
  const [busy, setBusy] = useState<"commit" | "push" | "pull" | "sync" | "fetch" | "merge" | "amend" | "undo" | "stash" | null>(null);
  const [editing, setEditing] = useState(false);
  const [editedMsg, setEditedMsg] = useState("");
  const [pushAfterCommit, setPushAfterCommit] = useState(false);
  const [claudeReady, setClaudeReady] = useState<boolean | null>(null);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const config = useAppStore((s) => s.appState?.config);
  const homeDir = useHomeDir();
  const aiEnabled = config?.ai_commit_message_enabled ?? true;
  const generation = useAiCommitStore((s) => s.getGeneration(workspace.workspace_id));
  const requestGeneration = useAiCommitStore((s) => s.requestGeneration);
  const consumeMessage = useAiCommitStore((s) => s.consumeMessage);
  const clearGeneration = useAiCommitStore((s) => s.clearGeneration);

  const diffSetFile = useDiffStore((s) => s.setFile);
  const diffInitTab = useDiffStore((s) => s.initTab);

  const staged = useMemo(() => files.filter((f) => f.is_staged), [files]);
  const unstaged = useMemo(() => files.filter((f) => f.is_unstaged), [files]);
  const conflicted = useMemo(() => files.filter((f) => f.status === "conflicted"), [files]);
  const isMerging = !!(mergeState?.is_merging || mergeState?.is_rebasing);
  const totalChanges = files.length;

  // Non-git project folder: `getGitStatus` errors (swallowed to `[]`), so
  // without this flag the panel would render a false "Working tree clean".
  // Instead we name the state and offer the explicit, opt-in `git init`.
  const showNoGit = showNoGitState(workspace, homeDir);
  const { initialize, initializing } = useInitializeGit(workspace);

  const refresh = useCallback(() => {
    if (!cwd) return;
    Promise.all([
      getGitStatus(cwd).catch(() => [] as GitFileStatus[]),
      getGitBranchInfo(cwd).catch(() => null),
      getMergeState(cwd).catch(() => null as MergeState | null),
    ]).then(([s, info, merge]) => {
      // This refresh runs every 10 s over three IPC calls, and a quiet
      // working tree returns the same payloads every time. Keeping the
      // previous objects when nothing moved makes the idle tick a React
      // bail-out instead of a full panel re-render.
      setFiles((prev) => keepIfUnchanged(prev, s));
      if (info) setBranchInfo((prev) => keepIfUnchanged(prev, info));
      setMergeState((prev) => keepIfUnchanged(prev, merge));
    });
  }, [cwd]);

  useEffect(() => {
    refresh();
    refreshRef.current = setInterval(refresh, 10_000);
    return () => {
      if (refreshRef.current) clearInterval(refreshRef.current);
    };
  }, [refresh]);

  // The pane bar's Refresh: fetch remote refs, then re-read status. Same
  // work the panel's old header button did, driven from the shared bar.
  useEffect(() => {
    if (refreshKey === 0 || !cwd) return;
    setBusy("fetch");
    gitFetchChanges(cwd)
      .catch(() => {})
      .finally(() => {
        setBusy(null);
        refresh();
      });
    // `refresh`/`cwd` are stable per workspace; the bump is the trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  useEffect(() => {
    if (aiEnabled) {
      checkClaudeAvailable().then(setClaudeReady).catch(() => setClaudeReady(false));
    }
  }, [aiEnabled]);

  // Invalidate Review-tab queries so the PR badge reflects a fresh
  // commit / push / pull immediately instead of waiting for the 60s
  // background poll.
  const invalidateReviewQueries = useCallback(() => {
    queryClient.invalidateQueries({
      predicate: (q) =>
        q.queryKey[0] === "pr" && q.queryKey[2] === workspace.workspace_id,
    });
  }, [queryClient, workspace.workspace_id]);

  const openDiff = useCallback(
    async (filePath: string, isStaged: boolean) => {
      if (onOpenDiffOverride) {
        onOpenDiffOverride(filePath, isStaged);
        return;
      }
      const existing = workspace.tabs.find((t) => t.kind === "diff");
      if (existing) {
        await activateTab(workspace.workspace_id, existing.tab_id).catch(console.error);
        diffSetFile(existing.tab_id, filePath, isStaged);
        return;
      }
      try {
        const tabId = await createTab(workspace.workspace_id, "diff");
        diffInitTab(tabId, { file: filePath, staged: isStaged });
      } catch (err) {
        console.error("Failed to create diff tab:", err);
      }
    },
    [workspace, diffSetFile, diffInitTab, onOpenDiffOverride],
  );

  // ── Commit flow ──
  //
  // One button does it all: stage anything unstaged, ask the AI for a
  // message, surface a preview the user can confirm or edit. If AI is
  // unavailable we drop straight into the textarea so the user always
  // has a path forward. `andPush` lets the dropdown's "Commit & Push"
  // option chain a push after the commit lands.
  const beginCommit = async (andPush: boolean = false) => {
    if (busy) return;
    if (totalChanges === 0) return;
    setPushAfterCommit(andPush);
    if (unstaged.length > 0 && staged.length === 0) {
      try {
        await gitStageFiles(cwd, unstaged.map((f) => f.path));
        refresh();
      } catch (err) {
        toast.error(`Stage failed: ${err}`);
        return;
      }
    }
    // An explicit commit-message CLI wins; otherwise the Utility agent
    // supplies the provider. Its model only rides along when the provider
    // matches, so an override CLI is never handed another provider's model
    // name (`claude --model <codex model>` fails every commit).
    const utility = utilitySelectionFromStores();
    const cli = config?.ai_commit_message_cli ?? utility?.provider ?? "claude";
    const model =
      config?.ai_commit_message_model ??
      (utility?.provider === cli ? utility.model : null);
    // Only claude has an availability preflight; when it says "missing" we
    // fall back to the manual textarea instead of a doomed spawn.
    const canUseAi = aiEnabled && (cli !== "claude" || claudeReady !== false);
    if (canUseAi) {
      requestGeneration(workspace.workspace_id, cwd, cli, model);
    } else {
      setEditedMsg("");
      setEditing(true);
    }
  };

  const finalizeCommit = async (msg: string) => {
    const trimmed = msg.trim();
    if (!trimmed) return;
    setBusy("commit");
    try {
      await gitCommitChanges(cwd, trimmed);
      clearGeneration(workspace.workspace_id);
      setEditing(false);
      setEditedMsg("");
      if (pushAfterCommit) {
        setBusy("push");
        try {
          await gitPushChanges(cwd, branchInfo ? !branchInfo.has_upstream : false);
        } catch (err) {
          toast.error(`Push after commit failed: ${err}`);
        }
        setPushAfterCommit(false);
      }
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Commit failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleAmend = async () => {
    if (busy) return;
    setBusy("amend");
    try {
      if (unstaged.length > 0 && staged.length === 0) {
        await gitStageFiles(cwd, unstaged.map((f) => f.path));
      }
      await gitAmendCommit(cwd, null);
      toast.success("Amended last commit");
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Amend failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleUndoLastCommit = async () => {
    if (busy) return;
    setBusy("undo");
    try {
      await gitUndoLastCommit(cwd);
      toast.success("Undid last commit (changes kept)");
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Undo failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleStashPush = async () => {
    if (busy) return;
    setBusy("stash");
    try {
      await gitStashPush(cwd, true);
      toast.success("Stashed changes");
      refresh();
    } catch (err) {
      toast.error(`Stash failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleStashPop = async () => {
    if (busy) return;
    setBusy("stash");
    try {
      await gitStashPop(cwd);
      toast.success("Popped stash");
      refresh();
    } catch (err) {
      toast.error(`Stash pop failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  // Watch AI generation — drop the result into the preview banner on
  // success, surface the error and reset on failure.
  useEffect(() => {
    if (!generation) return;
    if (generation.status === "error") {
      toast.error(generation.error ?? "Generation failed");
      clearGeneration(workspace.workspace_id);
    }
  }, [generation, clearGeneration, workspace.workspace_id]);

  const handlePush = async () => {
    if (busy) return;
    setBusy("push");
    try {
      await gitPushChanges(cwd, branchInfo ? !branchInfo.has_upstream : false);
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Push failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handlePull = async () => {
    if (busy) return;
    setBusy("pull");
    try {
      await gitPullChanges(cwd);
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Pull failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleSync = async () => {
    if (busy) return;
    setBusy("sync");
    try {
      await gitPullChanges(cwd);
      await gitPushChanges(cwd, false);
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Sync failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleFetch = async () => {
    if (busy) return;
    setBusy("fetch");
    try {
      await gitFetchChanges(cwd);
      refresh();
    } catch (err) {
      toast.error(`Fetch failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleAbortMerge = async () => {
    if (busy) return;
    setBusy("merge");
    try {
      await abortMerge(cwd);
      refresh();
    } catch (err) {
      toast.error(`Abort failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  const handleContinueMerge = async () => {
    if (busy) return;
    setBusy("merge");
    try {
      await continueMerge(cwd, "Merge commit");
      refresh();
      invalidateReviewQueries();
    } catch (err) {
      toast.error(`Continue failed: ${err}`);
    } finally {
      setBusy(null);
    }
  };

  // ── Render ──

  const generatedMsg = generation?.status === "done" ? generation.message ?? "" : "";
  const isGenerating = generation?.status === "generating";
  const showPreview = !!generatedMsg && !editing;
  const showEditor = editing;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* No header row: the title, the change count and Refresh moved to
          the deck's shared pane bar, which also carries the +N/−N totals. */}
      <BranchPill info={branchInfo} />

      {isMerging && (
        <div className="flex items-center gap-2 px-2.5 py-2 bg-warning/10 border-b border-warning/30">
          <GitMerge className="size-3.5 text-warning shrink-0" />
          <div className="flex-1 min-w-0">
            <p className="text-[11px] font-medium text-foreground">Merge in progress</p>
            {conflicted.length > 0 && (
              <p className="text-[10px] text-muted-foreground">
                {conflicted.length} conflict{conflicted.length === 1 ? "" : "s"} to resolve
              </p>
            )}
          </div>
          <Button
            variant="ghost"
            size="xs"
            className="h-6 text-[10px] text-muted-foreground hover:text-foreground"
            onClick={handleAbortMerge}
            disabled={busy !== null}
          >
            Abort
          </Button>
          {conflicted.length === 0 && (
            <Button
              size="xs"
              variant="ghost"
              className="h-6 text-[10px] bg-foreground/[0.08] hover:bg-foreground/[0.14] text-foreground border border-border/60"
              onClick={handleContinueMerge}
              disabled={busy !== null}
            >
              Continue
            </Button>
          )}
        </div>
      )}

      <ScrollArea className="flex-1 min-h-0">
        <div className="py-1">
          {totalChanges === 0 ? (
            showNoGit ? (
              <div className="flex flex-col items-center justify-center px-4 py-10 gap-2 text-center text-muted-foreground/70">
                <GitBranch className="size-4 opacity-50" />
                <p className="text-[11px]">
                  Not a git repository — changes can&apos;t be tracked
                </p>
                <Button
                  size="xs"
                  variant="ghost"
                  className="h-6 text-[10px] bg-foreground/[0.08] hover:bg-foreground/[0.14] text-foreground border border-border/60"
                  onClick={async () => {
                    await initialize();
                    refresh();
                  }}
                  disabled={initializing}
                >
                  {initializing ? "Initializing…" : "Initialize Git"}
                </Button>
              </div>
            ) : (
              <div className="flex flex-col items-center justify-center py-10 gap-1.5 text-muted-foreground/70">
                <Check className="size-4 opacity-50" />
                <p className="text-[11px]">Working tree clean</p>
              </div>
            )
          ) : (
            <>
              {(sectionFilter === "all" || sectionFilter === "staged") && (
                <FileSection
                  label="Staged"
                  files={staged}
                  staged
                  cwd={cwd}
                  onRefresh={refresh}
                  onOpenDiff={openDiff}
                />
              )}
              {(sectionFilter === "all" || sectionFilter === "unstaged") && (
                <FileSection
                  label="Changed"
                  files={unstaged.filter((f) => f.status !== "conflicted")}
                  staged={false}
                  cwd={cwd}
                  onRefresh={refresh}
                  onOpenDiff={openDiff}
                />
              )}
              {(sectionFilter === "all" || sectionFilter === "conflicts") && (
                <FileSection
                  label="Conflicts"
                  files={conflicted}
                  staged={false}
                  cwd={cwd}
                  onRefresh={refresh}
                  onOpenDiff={openDiff}
                />
              )}
            </>
          )}
        </div>
      </ScrollArea>

      {/* Action bar — the panel's single moment of weight. Holds the
          AI-message preview banner (when present) above the smart
          Commit button. Banner uses a left-rule pull-quote treatment
          so the generated text reads as an editorial artifact, not
          just another form field. Hidden for non-git folders — every
          action here (commit/push/pull/fetch) needs a repo, and the
          empty state above already offers Initialize Git. */}
      {!showNoGit && (
      <div className="shrink-0 border-t border-border/60 bg-card/40 p-2 space-y-2">
        {showPreview && (
          <div className="rounded-md border border-border/60 bg-background overflow-hidden">
            <div className="flex items-start gap-2 px-2.5 py-2 border-l-2 border-l-primary/70">
              <Sparkles className="size-3 text-primary/80 mt-0.5 shrink-0" />
              <p className="select-text flex-1 text-[11px] leading-snug text-foreground whitespace-pre-wrap break-words">
                {generatedMsg}
              </p>
            </div>
            <div className="flex items-center gap-1 px-1.5 py-1 border-t border-border/40">
              <Button
                size="xs"
                variant="ghost"
                className="h-6 text-[10px] flex-1 bg-foreground/[0.08] hover:bg-foreground/[0.14] text-foreground border border-border/60"
                onClick={() => finalizeCommit(generatedMsg)}
                disabled={busy !== null}
              >
                {busy === "commit" ? <Loader2 className="size-3 animate-spin" /> : "Commit"}
              </Button>
              <Button
                size="xs"
                variant="ghost"
                className="h-6 text-[10px] text-muted-foreground hover:text-foreground"
                onClick={() => {
                  const msg = consumeMessage(workspace.workspace_id) ?? generatedMsg;
                  setEditedMsg(msg);
                  setEditing(true);
                }}
                disabled={busy !== null}
              >
                Edit
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                className="text-muted-foreground hover:text-foreground"
                onClick={() => clearGeneration(workspace.workspace_id)}
                disabled={busy !== null}
                aria-label="Discard message"
              >
                <X className="size-3" />
              </Button>
            </div>
          </div>
        )}

        {showEditor && (
          <div className="rounded-md border border-border/60 bg-background overflow-hidden">
            <Textarea
              autoFocus
              value={editedMsg}
              onChange={(e) => setEditedMsg(e.target.value)}
              placeholder="Commit message"
              className="text-[11px] leading-snug resize-none border-0 rounded-none min-h-16 focus-visible:ring-0"
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  finalizeCommit(editedMsg);
                }
              }}
            />
            <div className="flex items-center gap-1 px-1.5 py-1 border-t border-border/40">
              <Button
                size="xs"
                variant="ghost"
                className="h-6 text-[10px] flex-1 bg-foreground/[0.08] hover:bg-foreground/[0.14] text-foreground border border-border/60"
                onClick={() => finalizeCommit(editedMsg)}
                disabled={!editedMsg.trim() || busy !== null}
              >
                {busy === "commit" ? <Loader2 className="size-3 animate-spin" /> : "Commit"}
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                className="text-muted-foreground hover:text-foreground"
                onClick={() => { setEditing(false); setEditedMsg(""); }}
                disabled={busy !== null}
                aria-label="Cancel"
              >
                <X className="size-3" />
              </Button>
            </div>
          </div>
        )}

        {!showPreview && !showEditor && (
          <SmartCommitButton
            hasChanges={totalChanges > 0}
            staged={staged.length}
            isGenerating={isGenerating ?? false}
            isMerging={isMerging}
            busy={busy}
            onCommit={() => beginCommit(false)}
            onCommitAndPush={() => beginCommit(true)}
            onAmend={handleAmend}
            onUndoLastCommit={handleUndoLastCommit}
            onStashPush={handleStashPush}
            onStashPop={handleStashPop}
            onPush={handlePush}
            onPull={handlePull}
            onSync={handleSync}
            onFetch={handleFetch}
            ahead={branchInfo?.ahead ?? 0}
            behind={branchInfo?.behind ?? 0}
          />
        )}
      </div>
      )}
    </div>
  );
}

// ── SmartCommitButton ──
//
// Primary action morphs with state: Commit when there's something to
// commit, Push when ahead, Pull when behind, Sync when both, Fetch when
// clean and even. Dropdown always exposes every action so power users
// aren't trapped by the heuristic.
function SmartCommitButton({
  hasChanges,
  staged,
  isGenerating,
  isMerging,
  busy,
  onCommit,
  onCommitAndPush,
  onAmend,
  onUndoLastCommit,
  onStashPush,
  onStashPop,
  onPush,
  onPull,
  onSync,
  onFetch,
  ahead,
  behind,
}: {
  hasChanges: boolean;
  staged: number;
  isGenerating: boolean;
  isMerging: boolean;
  busy: string | null;
  onCommit: () => void;
  onCommitAndPush: () => void;
  onAmend: () => void;
  onUndoLastCommit: () => void;
  onStashPush: () => void;
  onStashPop: () => void;
  onPush: () => void;
  onPull: () => void;
  onSync: () => void;
  onFetch: () => void;
  ahead: number;
  behind: number;
}) {
  const primary = (() => {
    if (isMerging) return null;
    if (hasChanges) {
      return {
        label: isGenerating ? "Writing message…" : staged > 0 ? `Commit ${staged}` : "Commit",
        icon: isGenerating ? <Loader2 className="size-3 animate-spin" /> : <Sparkles className="size-3" />,
        action: onCommit,
        disabled: isGenerating || busy !== null,
      };
    }
    if (ahead > 0 && behind > 0) {
      return { label: "Sync", icon: <ArrowUpDown className="size-3" />, action: onSync, disabled: busy !== null };
    }
    if (ahead > 0) {
      return { label: `Push ${ahead}`, icon: <ArrowUp className="size-3" />, action: onPush, disabled: busy !== null };
    }
    if (behind > 0) {
      return { label: `Pull ${behind}`, icon: <ArrowDown className="size-3" />, action: onPull, disabled: busy !== null };
    }
    return { label: "Fetch", icon: <Download className="size-3" />, action: onFetch, disabled: busy !== null };
  })();

  if (!primary) {
    return (
      <div className="text-[10px] text-muted-foreground/60 text-center py-1">
        Resolve or abort the merge above.
      </div>
    );
  }

  // Neutral fill matched to the panel chrome instead of the theme's
  // `--primary` accent — `--primary` is theme-defined (orange on
  // ember-dark, blue on default-dark) and the sidebar reads cleaner
  // when the action button blends with the card surface rather than
  // competing with it.
  const fillCls =
    "bg-foreground/[0.08] hover:bg-foreground/[0.14] text-foreground border border-border/60";

  return (
    <div className="flex items-stretch gap-px rounded-md overflow-hidden">
      <Button
        size="sm"
        variant="ghost"
        className={cn(
          "flex-1 h-8 text-xs gap-1.5 rounded-r-none border-r-0",
          fillCls,
        )}
        onClick={primary.action}
        disabled={primary.disabled}
      >
        {primary.icon}
        {primary.label}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            size="sm"
            variant="ghost"
            className={cn("h-8 w-7 px-0 rounded-l-none border-l-0", fillCls)}
            aria-label="More actions"
            disabled={busy !== null}
          >
            <ChevronDown className="size-3" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[200px]">
          {hasChanges && (
            <>
              <DropdownMenuItem onClick={onCommit} disabled={isGenerating}>
                <Sparkles className="size-3 mr-2" />
                Commit (AI message)
              </DropdownMenuItem>
              <DropdownMenuItem onClick={onCommitAndPush} disabled={isGenerating}>
                <Send className="size-3 mr-2" />
                Commit & Push
              </DropdownMenuItem>
              <DropdownMenuItem onClick={onAmend}>
                <Pencil className="size-3 mr-2" />
                Amend last commit
              </DropdownMenuItem>
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem onClick={onPush}>
            <ArrowUp className="size-3 mr-2" />
            Push {ahead > 0 && <span className="ml-auto text-[10px] tabular-nums text-foreground/70">{ahead}</span>}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onPull}>
            <ArrowDown className="size-3 mr-2" />
            Pull {behind > 0 && <span className="ml-auto text-[10px] tabular-nums text-foreground/70">{behind}</span>}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onSync}>
            <ArrowUpDown className="size-3 mr-2" />
            Sync
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onFetch}>
            <Download className="size-3 mr-2" />
            Fetch
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={onStashPush} disabled={!hasChanges}>
            <Archive className="size-3 mr-2" />
            Stash changes
          </DropdownMenuItem>
          <DropdownMenuItem onClick={onStashPop}>
            <ArchiveRestore className="size-3 mr-2" />
            Pop stash
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={onUndoLastCommit} className="text-muted-foreground focus:text-foreground">
            <Undo2 className="size-3 mr-2" />
            Undo last commit
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
