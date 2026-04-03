import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { cn } from "@/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { BranchPicker } from "./branch-picker";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  GitPullRequest,
  ArrowUp,
  ChevronDown,
  Check,
  Paperclip,
  X,
  CircleDot,
} from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { PresetIcon } from "@/components/icons/preset-icon";
import { ProjectPicker } from "./project-picker";
import { IssuePickerPanel } from "@/components/github/issue-picker";
import {
  listBranches,
  listBranchesDetailed,
  listWorktrees,
  getGitBranchInfo,
  createWorkspace,
  createWorktreeWorkspace,
  importWorktreeWorkspace,
  activateWorkspace,
  getPresets,
  checkIsGitRepo,
  dbAddRecentProject,
  generateBranchName,
  generateRandomBranchName,
  checkGhAvailable,
  checkGithubRepo,
  listPullRequests,
  pickFilesDialog,
  suggestIssueBranchName,
  linkWorkspaceIssue,
  getGithubIssue,
  getGithubIssueByPath,
} from "@/tauri/commands";
import type { TerminalPreset, WorktreeInfo, BranchDetail, PullRequestInfo, GitHubIssue, LinkedIssue } from "@/tauri/types";

const ISSUE_BODY_MAX_CHARS = 10_000;

/** Build a prompt with issue context prepended. Exported for testing. */
export function buildPromptWithIssueContext(
  userPrompt: string,
  issue: Pick<LinkedIssue, "number" | "title" | "state" | "labels"> | null,
  issueBody: string | null,
): string {
  if (!issue) return userPrompt;

  const lines: string[] = [
    "The following GitHub issue is linked to this workspace:",
    "",
    `Issue #${issue.number}: ${issue.title}`,
    `Status: ${issue.state}`,
  ];
  if (issue.labels.length > 0) {
    lines.push(`Labels: ${issue.labels.join(", ")}`);
  }
  if (issueBody) {
    const truncated =
      issueBody.length > ISSUE_BODY_MAX_CHARS
        ? issueBody.slice(0, ISSUE_BODY_MAX_CHARS) + "\n...[truncated]"
        : issueBody;
    lines.push("", "Description:", truncated);
  }
  lines.push("", "---", "");

  return lines.join("\n") + userPrompt;
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function NewWorkspaceDialog({ open, onOpenChange }: Props) {
  const appState = useAppStore((s) => s.appState);
  const activeWs = appState?.workspaces.find(
    (w) => w.workspace_id === appState.active_workspace_id,
  );
  const storeProjectDir = useUIStore((s) => s.newWorkspaceProjectDir);
  const lastSelectedAgentId = useUIStore((s) => s.lastSelectedAgentId);
  const addPendingWorkspace = useUIStore((s) => s.addPendingWorkspace);
  const removePendingWorkspace = useUIStore((s) => s.removePendingWorkspace);
  const failPendingWorkspace = useUIStore((s) => s.failPendingWorkspace);
  const setLastSelectedAgentId = useUIStore((s) => s.setLastSelectedAgentId);

  const defaultDir =
    storeProjectDir || activeWs?.project_root || activeWs?.cwd || "";

  // Form state
  const [projectDir, setProjectDir] = useState(defaultDir);
  const [workspaceName, setWorkspaceName] = useState("");
  const [branchName, setBranchName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(
    lastSelectedAgentId || "builtin-claude",
  );
  const [baseBranch, setBaseBranch] = useState("main");
  const [attachments, setAttachments] = useState<string[]>([]);
  const [linkedIssue, setLinkedIssue] = useState<GitHubIssue | null>(null);
  const [issuePickerOpen, setIssuePickerOpen] = useState(false);
  const [branchAutoFilled, setBranchAutoFilled] = useState(false);
  const [branchMode, setBranchMode] = useState<"create_new" | "open_existing">("create_new");
  const [openExistingBranch, setOpenExistingBranch] = useState<string | null>(null);

  // Data state
  const [presets, setPresets] = useState<TerminalPreset[]>([]);
  const [localBranches, setLocalBranches] = useState<string[]>([]);
  const [remoteBranches, setRemoteBranches] = useState<string[]>([]);
  const [detailedBranches, setDetailedBranches] = useState<BranchDetail[]>([]);
  const [branchesLoading, setBranchesLoading] = useState(false);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null);
  const [prBranches, setPrBranches] = useState<Set<string>>(new Set());
  const [openPrs, setOpenPrs] = useState<PullRequestInfo[]>([]);
  const [ghAvailable, setGhAvailable] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const issuePickerRef = useRef<HTMLDivElement>(null);

  // Reset state when dialog opens
  const prevOpenRef = useRef(false);
  if (open && !prevOpenRef.current) {
    const dir = storeProjectDir || activeWs?.project_root || activeWs?.cwd || "";
    if (projectDir !== dir) setProjectDir(dir);
    setWorkspaceName("");
    setBranchName("");
    setPrompt("");
    setSelectedAgentId(lastSelectedAgentId || "builtin-claude");
    setBaseBranch("main");
    setAttachments([]);
    setLinkedIssue(null);
    setIssuePickerOpen(false);
    setBranchAutoFilled(false);
    setBranchMode("create_new");
    setOpenExistingBranch(null);
  }
  prevOpenRef.current = open;

  // Load data when dialog opens or project changes
  useEffect(() => {
    if (!open || !projectDir) return;
    let cancelled = false;

    setIsGitRepo(null);
    setLocalBranches([]);
    setRemoteBranches([]);
    setDetailedBranches([]);
    setBranchesLoading(true);
    setPrBranches(new Set());

    checkIsGitRepo(projectDir).then((isRepo) => {
      if (cancelled) return;
      setIsGitRepo(isRepo);
      if (!isRepo) { setBranchesLoading(false); return; }

      Promise.all([
        listBranches(projectDir, false).catch(() => []),
        listBranches(projectDir, true).catch(() => []),
        listBranchesDetailed(projectDir).catch(() => []),
        listWorktrees(projectDir).catch(() => []),
        getGitBranchInfo(projectDir).catch(() => ({
          branch: null,
          ahead: 0,
          behind: 0,
        })),
      ]).then(([local, remote, detailed, wt, info]) => {
        if (cancelled) return;
        setLocalBranches(local);
        setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
        setDetailedBranches(detailed);
        setBranchesLoading(false);
        setWorktrees(wt);
        setCurrentBranch(info.branch);
        setBaseBranch(info.branch ?? "main");
      });

      // Fetch open PRs for branch badges and "+" menu (non-blocking)
      Promise.all([checkGhAvailable(), checkGithubRepo(projectDir)])
        .then(([available, isGhRepo]) => {
          if (cancelled) return;
          setGhAvailable(available && isGhRepo);
          if (!available || !isGhRepo) return;
          listPullRequests(projectDir, "open")
            .then((prs) => {
              if (cancelled) return;
              setOpenPrs(prs);
              const heads = new Set<string>();
              for (const pr of prs) {
                if (pr.head_branch) heads.add(pr.head_branch);
              }
              setPrBranches(heads);
            })
            .catch(() => {});
        })
        .catch(() => {});
    });

    // Fetch presets
    getPresets()
      .then((snap) => {
        if (!cancelled) setPresets(snap.presets.filter((p) => p.pinned));
      })
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [open, projectDir]);

  // Focus textarea when dialog opens
  useEffect(() => {
    if (open) {
      setTimeout(() => textareaRef.current?.focus(), 100);
    }
  }, [open]);

  // Branch workspace map (same project scope)
  const branchWorkspaceMap = useMemo(() => {
    const map = new Map<string, string>();
    if (appState && projectDir) {
      for (const ws of appState.workspaces) {
        if (
          ws.git_branch &&
          (ws.project_root === projectDir || ws.cwd === projectDir)
        ) {
          map.set(ws.git_branch, ws.workspace_id);
        }
      }
    }
    return map;
  }, [appState, projectDir]);

  // All branches merged and deduplicated
  const allBranches = useMemo(() => {
    const set = new Set([...localBranches, ...remoteBranches]);
    return Array.from(set).sort();
  }, [localBranches, remoteBranches]);

  // Find a workspace_id for the current project (needed by issue picker to resolve repo)
  const projectWorkspaceId = useMemo(() => {
    if (!appState || !projectDir) return null;
    const ws = appState.workspaces.find(
      (w) => w.project_root === projectDir || w.cwd === projectDir,
    );
    return ws?.workspace_id ?? null;
  }, [appState, projectDir]);

  // Whether the issue picker is available (needs gh + a GitHub repo)
  const isGithubRepo = ghAvailable;

  // Selected agent preset
  const selectedAgent = useMemo(
    () => presets.find((p) => p.id === selectedAgentId) ?? null,
    [presets, selectedAgentId],
  );

  // Auto-resize textarea (min 96px = ~5 lines, max 192px = ~10 lines)
  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setPrompt(e.target.value);
    const ta = e.target;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(Math.max(ta.scrollHeight, 40), 192)}px`;
  };

  const handleIssueSelect = useCallback(
    async (issue: GitHubIssue) => {
      setLinkedIssue(issue);
      // Auto-fill branch name if empty or if current name was auto-filled from a previous issue
      if (branchMode === "create_new" && (!branchName.trim() || branchAutoFilled)) {
        try {
          const suggested = await suggestIssueBranchName(issue.number, issue.title);
          setBranchName(suggested);
          setBranchAutoFilled(true);
        } catch {
          // Non-blocking — user can type their own
        }
      }
    },
    [branchName, branchAutoFilled, branchMode],
  );

  const handleOpenExisting = useCallback((branch: string) => {
    setBranchMode("open_existing");
    setOpenExistingBranch(branch);
    setBranchName("");
  }, []);

  const handleSubmit = useCallback(async () => {
    if (!projectDir) return;

    // Build user prompt with attachments
    let userPrompt = attachments.length > 0
      ? `${prompt.trim()}\n\nAttached files:\n${attachments.map((f) => `- ${f}`).join("\n")}`
      : prompt.trim();

    // Inject linked issue context into prompt
    if (linkedIssue) {
      let issueBody: string | null = null;
      try {
        const full = projectWorkspaceId
          ? await getGithubIssue(projectWorkspaceId, linkedIssue.number)
          : await getGithubIssueByPath(projectDir, linkedIssue.number);
        issueBody = full.body ?? null;
      } catch {
        // Non-blocking: proceed without body
      }
      userPrompt = buildPromptWithIssueContext(userPrompt, linkedIssue, issueBody);
    }

    const fullPrompt = userPrompt;

    // Close dialog immediately (optimistic)
    onOpenChange(false);

    // Generate a temporary ID for the pending workspace
    const tempId = crypto.randomUUID();
    const displayName =
      workspaceName || prompt.slice(0, 40) || openExistingBranch || branchName || "New workspace";

    addPendingWorkspace({
      id: tempId,
      name: displayName,
      projectPath: projectDir,
      status: "creating",
    });

    try {
      // Open existing branch mode — skip branch generation
      if (branchMode === "open_existing" && openExistingBranch) {
        const existingWsId = branchWorkspaceMap.get(openExistingBranch);
        if (existingWsId) {
          await activateWorkspace(existingWsId);
          removePendingWorkspace(tempId);
          return;
        }

        let wsId: string;
        const orphan = worktrees.find(
          (wt) =>
            wt.branch === openExistingBranch ||
            wt.branch === `refs/heads/${openExistingBranch}`,
        );

        if (orphan) {
          wsId = await importWorktreeWorkspace(orphan.path, openExistingBranch, "single");
        } else if (openExistingBranch === currentBranch) {
          wsId = await createWorkspace(projectDir);
        } else {
          wsId = await createWorktreeWorkspace(
            projectDir,
            openExistingBranch,
            false,
            "single",
            null,
            fullPrompt || null,
            selectedAgentId,
          );
        }

        const pName = projectDir.split("/").filter(Boolean).pop() || projectDir;
        dbAddRecentProject(projectDir, pName).catch(console.error);
        if (linkedIssue) {
          linkWorkspaceIssue(wsId, linkedIssue.number).catch(console.error);
        }
        removePendingWorkspace(tempId);
        await activateWorkspace(wsId);
        return;
      }

      // Determine branch name
      let resolvedBranch = branchName.trim();
      let isNewBranch = true;

      if (!resolvedBranch) {
        if (fullPrompt) {
          // AI-generated branch name from prompt
          resolvedBranch = await generateBranchName(prompt, projectDir);
        } else {
          // Random branch name
          resolvedBranch = await generateRandomBranchName(projectDir);
        }
      } else {
        // User provided a branch name — check if it's an existing branch
        if (allBranches.includes(resolvedBranch)) {
          isNewBranch = false;
        }
      }

      // Check if workspace already exists for this branch
      const existingWsId = branchWorkspaceMap.get(resolvedBranch);
      if (existingWsId) {
        await activateWorkspace(existingWsId);
        removePendingWorkspace(tempId);
        return;
      }

      let wsId: string;

      // Current branch: open as workspace directly
      if (resolvedBranch === currentBranch && !isNewBranch) {
        wsId = await createWorkspace(projectDir);
      } else {
        // Check for orphan worktree
        const orphan = worktrees.find(
          (wt) =>
            wt.branch === resolvedBranch ||
            wt.branch === `refs/heads/${resolvedBranch}`,
        );

        if (orphan) {
          wsId = await importWorktreeWorkspace(
            orphan.path,
            resolvedBranch,
            "single",
          );
        } else {
          wsId = await createWorktreeWorkspace(
            projectDir,
            resolvedBranch,
            isNewBranch,
            "single",
            isNewBranch ? baseBranch || null : null,
            fullPrompt || null,
            selectedAgentId,
          );
        }
      }

      // Track as recent project
      const pName =
        projectDir.split("/").filter(Boolean).pop() || projectDir;
      dbAddRecentProject(projectDir, pName).catch(console.error);

      // Link issue to the new workspace (non-blocking)
      if (linkedIssue) {
        linkWorkspaceIssue(wsId, linkedIssue.number).catch(console.error);
      }

      removePendingWorkspace(tempId);
      await activateWorkspace(wsId);
    } catch (err) {
      failPendingWorkspace(tempId, String(err));
      // Auto-remove failed entry after 5 seconds
      setTimeout(() => removePendingWorkspace(tempId), 5000);
    }
  }, [
    projectDir,
    workspaceName,
    branchName,
    prompt,
    attachments,
    selectedAgentId,
    baseBranch,
    allBranches,
    branchMode,
    openExistingBranch,
    branchWorkspaceMap,
    worktrees,
    currentBranch,
    linkedIssue,
    projectWorkspaceId,
    onOpenChange,
    addPendingWorkspace,
    removePendingWorkspace,
    failPendingWorkspace,
  ]);

  // Close issue picker on click outside it (within the dialog)
  useEffect(() => {
    if (!issuePickerOpen) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (issuePickerRef.current && !issuePickerRef.current.contains(e.target as Node)) {
        setIssuePickerOpen(false);
      }
    };
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [issuePickerOpen]);

  // Handle Ctrl+Enter
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="sm:max-w-[560px] max-h-[min(70vh,600px)] !top-[calc(50%-min(35vh,300px))] !-translate-y-0 bg-popover p-0 gap-0 overflow-visible"
        onKeyDown={handleKeyDown}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>New Workspace</DialogTitle>
          <DialogDescription>
            Create a new workspace from a prompt
          </DialogDescription>
        </DialogHeader>

        {/* Top row: workspace name + branch name — nearly invisible inline labels */}
        <div className="flex gap-3 px-4 pt-3 pb-0.5">
          <Input
            value={workspaceName}
            onChange={(e) => setWorkspaceName(e.target.value)}
            placeholder="Workspace name (optional)"
            className="h-6 text-xs flex-1 border-0 bg-transparent dark:bg-transparent px-0 shadow-none focus-visible:ring-0 text-muted-foreground placeholder:text-muted-foreground/40"
          />
          {branchMode === "create_new" ? (
            <Input
              value={branchName}
              onChange={(e) => { setBranchName(e.target.value); setBranchAutoFilled(false); }}
              placeholder="branch name"
              className="h-6 text-xs w-[140px] border-0 bg-transparent dark:bg-transparent px-0 shadow-none focus-visible:ring-0 text-right font-mono text-muted-foreground placeholder:text-muted-foreground/40"
            />
          ) : (
            <span className="h-6 text-xs text-right font-mono text-muted-foreground/60 flex items-center truncate max-w-[180px]">
              on {openExistingBranch}
            </span>
          )}
        </div>

        {/* Center: prompt textarea with embedded controls */}
        <div className="relative px-3 pt-2 pb-3">
          <div className="rounded-2xl border border-border bg-muted overflow-hidden">
            <Textarea
              ref={textareaRef}
              value={prompt}
              onChange={handleTextareaChange}
              placeholder="What do you want to do?"
              className="min-h-10 max-h-48 resize-none border-0 bg-transparent dark:bg-transparent shadow-none focus-visible:ring-0 text-sm px-4 pt-3 pb-1"
              rows={1}
            />

            {/* Attachment chips + linked issue chip */}
            {(attachments.length > 0 || linkedIssue) && (
              <div className="flex flex-wrap gap-1.5 px-4 pb-2">
                {/* Linked issue chip */}
                {linkedIssue && (
                  <span className="inline-flex items-center gap-1.5 rounded-full bg-muted px-2.5 py-0.5 text-[11px] text-muted-foreground">
                    <span
                      className={cn(
                        "size-1.5 rounded-full shrink-0",
                        linkedIssue.state === "Open" ? "bg-success" : "bg-muted-foreground",
                      )}
                    />
                    <span className="font-mono tabular-nums">#{linkedIssue.number}</span>
                    <span className="max-w-[180px] truncate">{linkedIssue.title}</span>
                    <button
                      type="button"
                      className="ml-0.5 rounded-full p-0.5 hover:bg-foreground/10 transition-colors"
                      onClick={() => {
                        setLinkedIssue(null);
                        if (branchAutoFilled) {
                          setBranchName("");
                          setBranchAutoFilled(false);
                        }
                      }}
                    >
                      <X className="h-2.5 w-2.5" />
                    </button>
                  </span>
                )}
                {attachments.map((file) => (
                  <span
                    key={file}
                    className="inline-flex items-center gap-1 rounded-full bg-muted px-2.5 py-0.5 text-[11px] text-muted-foreground"
                  >
                    <Paperclip className="h-2.5 w-2.5" />
                    <span className="max-w-[160px] truncate">
                      {file.split("/").pop()}
                    </span>
                    <button
                      type="button"
                      className="ml-0.5 rounded-full p-0.5 hover:bg-foreground/10 transition-colors"
                      onClick={() =>
                        setAttachments((prev) => prev.filter((f) => f !== file))
                      }
                    >
                      <X className="h-2.5 w-2.5" />
                    </button>
                  </span>
                ))}
              </div>
            )}

            {/* Footer inside textarea border */}
            <div className="flex items-center justify-between px-3 pb-3 pt-0">
              {/* Agent picker — pill with real icon */}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-[11px] text-foreground transition-colors outline-none hover:bg-muted"
                  >
                    {selectedAgent ? (
                      <>
                        <PresetIcon icon={selectedAgent.icon} className="h-3.5 w-3.5" />
                        {selectedAgent.name}
                      </>
                    ) : (
                      <>
                        <PresetIcon icon="claude" className="h-3.5 w-3.5" />
                        Claude Code
                      </>
                    )}
                    <ChevronDown className="h-2.5 w-2.5 opacity-40" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="w-[200px]">
                  {presets.map((p) => (
                    <DropdownMenuItem
                      key={p.id}
                      onClick={() => {
                        setSelectedAgentId(p.id);
                        setLastSelectedAgentId(p.id);
                      }}
                      className="text-xs gap-2"
                    >
                      <PresetIcon icon={p.icon} className="h-3.5 w-3.5" />
                      <span className="flex-1">{p.name}</span>
                      {selectedAgentId === p.id && (
                        <Check className="h-3.5 w-3.5 text-primary" />
                      )}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>

              <div className="flex items-center gap-1">
                {/* Attach files */}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      aria-label="Attach files"
                      className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
                      onClick={async () => {
                        const files = await pickFilesDialog("Attach files");
                        if (files.length > 0) {
                          setAttachments((prev) => {
                            const existing = new Set(prev);
                            return [...prev, ...files.filter((f) => !existing.has(f))];
                          });
                        }
                      }}
                    >
                      <Paperclip className="h-4 w-4" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="top">Attach files</TooltipContent>
                </Tooltip>

                {/* Link pull request */}
                {ghAvailable && (
                  <DropdownMenu>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <DropdownMenuTrigger asChild>
                          <button
                            type="button"
                            aria-label="Link pull request"
                            className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
                          >
                            <GitPullRequest className="h-4 w-4" />
                          </button>
                        </DropdownMenuTrigger>
                      </TooltipTrigger>
                      <TooltipContent side="top">Link pull request</TooltipContent>
                    </Tooltip>
                    <DropdownMenuContent align="end" className="w-[260px] max-h-[240px] overflow-y-auto">
                      {openPrs.length === 0 ? (
                        <DropdownMenuItem disabled className="text-xs text-muted-foreground">
                          No open pull requests
                        </DropdownMenuItem>
                      ) : (
                        openPrs.map((pr) => (
                          <DropdownMenuItem
                            key={pr.number}
                            className="text-xs gap-2"
                            disabled={!pr.head_branch}
                            onClick={() => {
                              if (pr.head_branch) setBranchName(pr.head_branch);
                            }}
                          >
                            <div className="min-w-0 flex-1">
                              <div className="truncate">{pr.title}</div>
                              <div className="text-[10px] text-muted-foreground flex items-center gap-1">
                                <span>#{pr.number}</span>
                                {pr.head_branch && (
                                  <>
                                    <span className="opacity-40">-</span>
                                    <span className="truncate font-mono">{pr.head_branch}</span>
                                  </>
                                )}
                              </div>
                            </div>
                          </DropdownMenuItem>
                        ))
                      )}
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}

                {/* Link issue */}
                {isGithubRepo && !linkedIssue && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        aria-label="Link issue"
                        className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
                        onClick={() => setIssuePickerOpen(true)}
                      >
                        <CircleDot className="h-4 w-4" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="top">Link issue</TooltipContent>
                  </Tooltip>
                )}

                {/* Submit */}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      aria-label="Create"
                      className="inline-flex size-8 items-center justify-center rounded-full border border-border bg-muted text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50 outline-none"
                      onClick={handleSubmit}
                      disabled={!projectDir}
                    >
                      <ArrowUp className="h-4 w-4" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="top">Create workspace</TooltipContent>
                </Tooltip>
              </div>
            </div>
          </div>

          {/* Issue picker — absolute within the relative textarea area, floats below */}
          {issuePickerOpen && projectDir && (
            <div
              ref={issuePickerRef}
              className="absolute right-0 top-full mt-1 z-50 w-[320px] rounded-lg border border-border bg-popover shadow-lg overflow-hidden animate-in fade-in-0 zoom-in-95 duration-150"
            >
              <IssuePickerPanel
                workspaceId={projectWorkspaceId ?? undefined}
                projectPath={projectDir}
                open={issuePickerOpen}
                onSelect={handleIssueSelect}
                onClose={() => setIssuePickerOpen(false)}
              />
            </div>
          )}
        </div>

        {/* Bottom row: project + branch pickers as muted pills */}
        <div className="flex items-center gap-2 px-4 pb-3">
          <ProjectPicker
            value={projectDir || null}
            onChange={(path) => setProjectDir(path)}
          />

          {/* Base branch picker */}
          {isGitRepo !== false && (
            <BranchPicker
              baseBranch={openExistingBranch || baseBranch}
              branches={detailedBranches}
              worktrees={worktrees}
              branchWorkspaceMap={branchWorkspaceMap}
              prBranches={prBranches}
              currentBranch={currentBranch}
              loading={branchesLoading}
              onSelectBase={(branch) => {
                setBaseBranch(branch);
                setBranchMode("create_new");
                setOpenExistingBranch(null);
              }}
              onOpenWorkspace={(wsId) => {
                onOpenChange(false);
                activateWorkspace(wsId).catch(console.error);
              }}
              onImportWorktree={(path, branch) => {
                onOpenChange(false);
                importWorktreeWorkspace(path, branch, "single")
                  .then((wsId) => activateWorkspace(wsId))
                  .catch(console.error);
              }}
              onCreateOnCurrent={() => {
                onOpenChange(false);
                createWorkspace(projectDir)
                  .then((wsId) => activateWorkspace(wsId))
                  .catch(console.error);
              }}
              onOpenExisting={handleOpenExisting}
              isOpenMode={branchMode === "open_existing"}
            />
          )}

          <span className="ml-auto text-[10px] text-muted-foreground/40 select-none">
            Ctrl+Enter to create
          </span>
        </div>

      </DialogContent>
    </Dialog>
  );
}
