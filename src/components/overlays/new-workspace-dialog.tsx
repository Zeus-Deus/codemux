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
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Textarea } from "@/components/ui/textarea";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  GitBranch,
  GitPullRequest,
  Loader2,
  ArrowUp,
  ChevronDown,
  Plus,
  Check,
  Paperclip,
  X,
} from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { PresetIcon } from "@/components/icons/preset-icon";
import { ProjectPicker } from "./project-picker";
import {
  listBranches,
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
} from "@/tauri/commands";
import type { TerminalPreset, WorktreeInfo, PullRequestInfo } from "@/tauri/types";

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

  // Data state
  const [presets, setPresets] = useState<TerminalPreset[]>([]);
  const [localBranches, setLocalBranches] = useState<string[]>([]);
  const [remoteBranches, setRemoteBranches] = useState<string[]>([]);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null);
  const [prBranches, setPrBranches] = useState<Set<string>>(new Set());
  const [openPrs, setOpenPrs] = useState<PullRequestInfo[]>([]);
  const [ghAvailable, setGhAvailable] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);

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
  }
  prevOpenRef.current = open;

  // Load data when dialog opens or project changes
  useEffect(() => {
    if (!open || !projectDir) return;
    let cancelled = false;

    setIsGitRepo(null);
    setLocalBranches([]);
    setRemoteBranches([]);
    setPrBranches(new Set());

    checkIsGitRepo(projectDir).then((isRepo) => {
      if (cancelled) return;
      setIsGitRepo(isRepo);
      if (!isRepo) return;

      Promise.all([
        listBranches(projectDir, false).catch(() => []),
        listBranches(projectDir, true).catch(() => []),
        listWorktrees(projectDir).catch(() => []),
        getGitBranchInfo(projectDir).catch(() => ({
          branch: null,
          ahead: 0,
          behind: 0,
        })),
      ]).then(([local, remote, wt, info]) => {
        if (cancelled) return;
        setLocalBranches(local);
        setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
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

  const handleSubmit = useCallback(async () => {
    if (!projectDir) return;

    // Build full prompt with attachments
    const fullPrompt = attachments.length > 0
      ? `${prompt.trim()}\n\nAttached files:\n${attachments.map((f) => `- ${f}`).join("\n")}`
      : prompt.trim();

    // Close dialog immediately (optimistic)
    onOpenChange(false);

    // Generate a temporary ID for the pending workspace
    const tempId = crypto.randomUUID();
    const displayName =
      workspaceName || prompt.slice(0, 40) || branchName || "New workspace";

    addPendingWorkspace({
      id: tempId,
      name: displayName,
      projectPath: projectDir,
      status: "creating",
    });

    try {
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
    branchWorkspaceMap,
    worktrees,
    currentBranch,
    onOpenChange,
    addPendingWorkspace,
    removePendingWorkspace,
    failPendingWorkspace,
  ]);

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
        className="sm:max-w-[560px] max-h-[min(70vh,600px)] !top-[calc(50%-min(35vh,300px))] !-translate-y-0 bg-popover p-0 gap-0 overflow-hidden"
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
          <Input
            value={branchName}
            onChange={(e) => setBranchName(e.target.value)}
            placeholder="branch name"
            className="h-6 text-xs w-[140px] border-0 bg-transparent dark:bg-transparent px-0 shadow-none focus-visible:ring-0 text-right font-mono text-muted-foreground placeholder:text-muted-foreground/40"
          />
        </div>

        {/* Center: prompt textarea with embedded controls */}
        <div className="px-3 pt-2 pb-3">
          <div className="rounded-2xl border border-border bg-muted overflow-hidden">
            <Textarea
              ref={textareaRef}
              value={prompt}
              onChange={handleTextareaChange}
              placeholder="What do you want to do?"
              className="min-h-10 max-h-48 resize-none border-0 bg-transparent dark:bg-transparent shadow-none focus-visible:ring-0 text-sm px-4 pt-3 pb-1"
              rows={1}
            />

            {/* Attachment chips */}
            {attachments.length > 0 && (
              <div className="flex flex-wrap gap-1.5 px-4 pb-2">
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

              <div className="flex items-center gap-1.5">
                {/* "+" menu button — attach files, link PR */}
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      className="inline-flex size-8 items-center justify-center rounded-full border border-border bg-muted/60 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none"
                    >
                      <Plus className="h-4 w-4" />
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="w-[220px]">
                    <DropdownMenuItem
                      className="text-xs"
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
                      <Paperclip className="mr-2 h-3.5 w-3.5" />
                      Add attachment
                    </DropdownMenuItem>

                    {/* Link pull request — sub-menu with PR list */}
                    <DropdownMenuSub>
                      <DropdownMenuSubTrigger className="text-xs">
                        <GitPullRequest className="mr-2 h-3.5 w-3.5" />
                        Link pull request
                      </DropdownMenuSubTrigger>
                      <DropdownMenuSubContent className="w-[260px] max-h-[240px] overflow-y-auto">
                        {!ghAvailable ? (
                          <DropdownMenuItem disabled className="text-xs text-muted-foreground">
                            GitHub CLI not available
                          </DropdownMenuItem>
                        ) : openPrs.length === 0 ? (
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
                      </DropdownMenuSubContent>
                    </DropdownMenuSub>
                  </DropdownMenuContent>
                </DropdownMenu>

                {/* Submit button — circular icon, muted style */}
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
        </div>

        {/* Bottom row: project + branch pickers as muted pills */}
        <div className="flex items-center gap-2 px-4 pb-3">
          <ProjectPicker
            value={projectDir || null}
            onChange={(path) => setProjectDir(path)}
          />

          {/* Base branch picker */}
          {isGitRepo !== false && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1.5 rounded-full bg-muted/60 px-3 text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
                  disabled={allBranches.length === 0}
                >
                  <GitBranch className="h-3 w-3" />
                  <span className="max-w-[100px] truncate">{baseBranch}</span>
                  <ChevronDown className="h-2.5 w-2.5 opacity-40" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                className="max-h-[240px] w-[240px] overflow-y-auto"
              >
                {allBranches.map((branch) => {
                  const isDefault =
                    branch === "main" || branch === "master";
                  const hasWs = branchWorkspaceMap.has(branch);
                  const hasPr = prBranches.has(branch);
                  return (
                    <DropdownMenuItem
                      key={branch}
                      onClick={() => setBaseBranch(branch)}
                      className={cn(
                        "text-xs gap-2",
                        baseBranch === branch && "bg-accent",
                      )}
                    >
                      <span className="truncate flex-1">{branch}</span>
                      {hasPr && (
                        <Badge
                          variant="secondary"
                          className="text-[9px] px-1 py-0 bg-purple-500/15 text-purple-400 border-purple-500/20"
                        >
                          PR
                        </Badge>
                      )}
                      {isDefault && (
                        <Badge
                          variant="secondary"
                          className="text-[9px] px-1 py-0"
                        >
                          default
                        </Badge>
                      )}
                      {hasWs && (
                        <Badge
                          variant="outline"
                          className="text-[9px] px-1 py-0"
                        >
                          open
                        </Badge>
                      )}
                    </DropdownMenuItem>
                  );
                })}
                {allBranches.length === 0 && (
                  <DropdownMenuItem disabled className="text-xs">
                    <Loader2 className="mr-2 h-3 w-3 animate-spin" />
                    Loading...
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}

          <span className="ml-auto text-[10px] text-muted-foreground/40 select-none">
            Ctrl+Enter to create
          </span>
        </div>
      </DialogContent>
    </Dialog>
  );
}
