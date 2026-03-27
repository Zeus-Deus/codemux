import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { cn } from "@/lib/utils";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  GitBranch,
  GitPullRequest,
  Plus,
  Loader2,
  AlertCircle,
  FolderOpen,
} from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import {
  listBranches,
  listWorktrees,
  getGitBranchInfo,
  createWorkspace,
  createWorktreeWorkspace,
  importWorktreeWorkspace,
  activateWorkspace,
  getPresets,
  applyPreset,
  checkGhAvailable,
  checkGithubRepo,
  listPullRequests,
  pickFolderDialog,
  checkIsGitRepo,
  dbAddRecentProject,
  initGitRepo,
} from "@/tauri/commands";
import type {
  TerminalPreset,
  PullRequestInfo,
  WorktreeInfo,
} from "@/tauri/types";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function PresetSelector({
  presets,
  selected,
  onSelect,
}: {
  presets: TerminalPreset[];
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  if (presets.length === 0) return null;
  return (
    <div className="space-y-1.5">
      <label className="text-xs text-muted-foreground">Preset</label>
      <div className="flex flex-wrap gap-1.5">
        {presets.map((p) => (
          <Button
            key={p.id}
            variant={selected === p.id ? "secondary" : "outline"}
            size="sm"
            className={selected === p.id ? "border-primary bg-primary/10" : ""}
            onClick={() => onSelect(selected === p.id ? null : p.id)}
            aria-pressed={selected === p.id}
          >
            {p.name}
          </Button>
        ))}
      </div>
    </div>
  );
}

export function NewWorkspaceDialog({ open, onOpenChange }: Props) {
  const appState = useAppStore((s) => s.appState);
  const activeWs = appState?.workspaces.find(
    (w) => w.workspace_id === appState.active_workspace_id,
  );
  // Use the project dir passed from the "+" button if available,
  // otherwise fall back to the active workspace's project root or cwd.
  const storeProjectDir = useUIStore((s) => s.newWorkspaceProjectDir);
  const defaultDir = storeProjectDir || activeWs?.project_root || activeWs?.cwd || "";

  // Project directory (editable, defaults to the target project root)
  const [projectDir, setProjectDir] = useState(defaultDir);

  // Synchronously reset projectDir when dialog opens.
  // This runs during render (before effects) so the fetch effect
  // always sees the correct directory — no two-effect race condition.
  const prevOpenRef = useRef(false);
  if (open && !prevOpenRef.current && projectDir !== (defaultDir || "")) {
    setProjectDir(defaultDir || "");
  }
  prevOpenRef.current = open;

  const handlePickFolder = async () => {
    const folder = await pickFolderDialog("Choose project folder");
    if (folder) setProjectDir(folder);
  };

  // Tab selection — default to "existing" when branches exist
  const [activeTab, setActiveTab] = useState("new-branch");

  // Git repo detection
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null);
  const [initializing, setInitializing] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  const handleInitGit = async () => {
    if (!projectDir || initializing) return;
    setInitializing(true);
    setError(null);
    try {
      await initGitRepo(projectDir);
      setIsGitRepo(true);
      setReloadKey((k) => k + 1);
    } catch (err) {
      setError(String(err));
    } finally {
      setInitializing(false);
    }
  };

  // Shared state
  const [presets, setPresets] = useState<TerminalPreset[]>([]);
  const [selectedPreset, setSelectedPreset] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Branch state
  const [localBranches, setLocalBranches] = useState<string[]>([]);
  const [remoteBranches, setRemoteBranches] = useState<string[]>([]);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [branchSearch, setBranchSearch] = useState("");
  const [selectedBranch, setSelectedBranch] = useState<string | null>(null);
  const [newBranchName, setNewBranchName] = useState("");
  const [baseBranch, setBaseBranch] = useState("");

  // PR state
  const [prs, setPrs] = useState<PullRequestInfo[]>([]);
  const [ghAvailable, setGhAvailable] = useState(false);
  const [prLoading, setPrLoading] = useState(false);

  // Load data when dialog opens or project directory changes
  useEffect(() => {
    if (!open || !projectDir) return;
    let cancelled = false;

    setError(null);
    setCreating(false);
    setNewBranchName("");
    setBranchSearch("");
    setSelectedBranch(null);
    setIsGitRepo(null);
    setLocalBranches([]);
    setRemoteBranches([]);
    setPrs([]);

    // Check git status first
    checkIsGitRepo(projectDir).then((isRepo) => {
      if (cancelled) return;
      setIsGitRepo(isRepo);
      if (!isRepo) return;

      // Fetch branches
      Promise.all([
        listBranches(projectDir, false).catch(() => []),
        listBranches(projectDir, true).catch(() => []),
        listWorktrees(projectDir).catch(() => []),
        getGitBranchInfo(projectDir).catch(() => ({ branch: null, ahead: 0, behind: 0 })),
      ]).then(([local, remote, wt, info]) => {
        if (cancelled) return;
        setLocalBranches(local);
        setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
        setWorktrees(wt);
        setCurrentBranch(info.branch);
        setBaseBranch(info.branch ?? "main");
        // Default to "existing" tab when branches are available
        if (local.length > 0 || remote.length > 0) {
          setActiveTab("existing");
        }
      });

      // Check gh
      Promise.all([checkGhAvailable(), checkGithubRepo(projectDir)])
        .then(([available, isGhRepo]) => {
          if (cancelled) return;
          setGhAvailable(available && isGhRepo);
          if (available && isGhRepo) {
            setPrLoading(true);
            listPullRequests(projectDir, "open")
              .then((p) => { if (!cancelled) setPrs(p); })
              .catch(() => { if (!cancelled) setPrs([]); })
              .finally(() => { if (!cancelled) setPrLoading(false); });
          }
        })
        .catch(() => { if (!cancelled) setGhAvailable(false); });
    });

    // Fetch presets (independent of git status)
    getPresets()
      .then((snap) => { if (!cancelled) setPresets(snap.presets.filter((p) => p.pinned)); })
      .catch(() => {});

    return () => { cancelled = true; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, projectDir, reloadKey]);

  // Merged branch list (deduplicated)
  const allBranches = useMemo(() => {
    const set = new Set([...localBranches, ...remoteBranches]);
    return Array.from(set).sort();
  }, [localBranches, remoteBranches]);

  const filteredBranches = useMemo(() => {
    if (!branchSearch) return allBranches;
    const q = branchSearch.toLowerCase();
    return allBranches.filter((b) => b.toLowerCase().includes(q));
  }, [allBranches, branchSearch]);

  // Check if branch already has a workspace in the SAME project.
  // Different projects can have branches with the same name (e.g. "main").
  // Scope to projectDir (the dialog's target project), not the active workspace,
  // because the user may have changed the project directory in the dialog input.
  const branchWorkspaceMap = useMemo(() => {
    const map = new Map<string, string>();
    if (appState && projectDir) {
      for (const ws of appState.workspaces) {
        if (ws.git_branch && (ws.project_root === projectDir || ws.cwd === projectDir)) {
          map.set(ws.git_branch, ws.workspace_id);
        }
      }
    }
    return map;
  }, [appState, projectDir]);

  const handleCreate = useCallback(
    async (branch: string, isNewBranch: boolean) => {
      if (!projectDir || creating) return;
      setCreating(true);
      setError(null);

      try {
        // Check if workspace already exists for this branch
        const existingWsId = branchWorkspaceMap.get(branch);
        if (existingWsId) {
          await activateWorkspace(existingWsId);
          onOpenChange(false);
          return;
        }

        let wsId: string;

        // Current branch: open as workspace directly (no worktree needed)
        if (branch === currentBranch && !isNewBranch) {
          wsId = await createWorkspace(projectDir);
        } else {
          // Check for orphan worktree
          const orphan = worktrees.find(
            (wt) => wt.branch === branch || wt.branch === `refs/heads/${branch}`,
          );

          if (orphan) {
            wsId = await importWorktreeWorkspace(orphan.path, branch, "single");
          } else {
            wsId = await createWorktreeWorkspace(
              projectDir,
              branch,
              isNewBranch,
              "single",
              isNewBranch ? baseBranch || null : null,
            );
          }
        }

        // Apply preset if selected
        if (selectedPreset) {
          await applyPreset(wsId, selectedPreset, "split_pane").catch(
            console.error,
          );
        }

        // Track as recent project
        const projectName = projectDir.split("/").filter(Boolean).pop() || projectDir;
        dbAddRecentProject(projectDir, projectName).catch(console.error);

        onOpenChange(false);
      } catch (err) {
        setError(String(err));
      } finally {
        setCreating(false);
      }
    },
    [projectDir, creating, selectedPreset, baseBranch, branchWorkspaceMap, worktrees, onOpenChange],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[500px] sm:max-w-[500px] h-[440px] max-h-[85vh] flex flex-col p-0 gap-0 overflow-hidden">
        <DialogHeader className="p-4 pb-0 shrink-0">
          <DialogTitle className="text-sm">New Workspace</DialogTitle>
          <DialogDescription className="sr-only">Create a new workspace from a branch or pull request</DialogDescription>
        </DialogHeader>

        <div className="px-4 pt-3 space-y-1.5 shrink-0">
          <label className="text-xs text-muted-foreground">Project Directory</label>
          <div className="flex gap-1.5">
            <Input
              value={projectDir}
              onChange={(e) => setProjectDir(e.target.value)}
              className="h-8 text-xs flex-1"
              placeholder={defaultDir || "Select a project folder"}
            />
            <Button variant="outline" size="sm" className="h-8 shrink-0" onClick={handlePickFolder}>
              <FolderOpen className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>

        {isGitRepo === false ? (
          <div className="px-4 py-6 flex flex-col items-center gap-3">
            <AlertCircle className="h-8 w-8 text-muted-foreground/40" />
            <p className="text-sm text-muted-foreground">
              This folder is not a git repository
            </p>
            <Button
              variant="outline"
              size="sm"
              onClick={handleInitGit}
              disabled={initializing}
            >
              {initializing && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              Initialize Git Repository
            </Button>
          </div>
        ) : (
        <>
        <Tabs value={activeTab} onValueChange={setActiveTab} className="flex flex-col flex-1 min-h-0">
          <TabsList variant="line" className="mx-4 mt-2 h-8">
            <TabsTrigger value="new-branch" className="text-xs px-2 gap-1">
              <Plus className="h-3 w-3" /> New Branch
            </TabsTrigger>
            <TabsTrigger value="existing" className="text-xs px-2 gap-1">
              <GitBranch className="h-3 w-3" /> Existing
            </TabsTrigger>
            <TabsTrigger value="pr" className="text-xs px-2 gap-1">
              <GitPullRequest className="h-3 w-3" /> Pull Request
            </TabsTrigger>
          </TabsList>

          {/* ── New Branch ── */}
          <TabsContent value="new-branch" className="px-4 py-3 space-y-3 min-h-0 overflow-y-auto">
            <div className="space-y-1.5">
              <label className="text-xs text-muted-foreground">
                Branch name
              </label>
              <Input
                placeholder="feature/my-feature"
                value={newBranchName}
                onChange={(e) => setNewBranchName(e.target.value)}
                className="h-8 text-sm"
                autoFocus
              />
            </div>
            <div className="space-y-1.5">
              <label className="text-xs text-muted-foreground">
                Base branch
              </label>
              <Select value={baseBranch} onValueChange={setBaseBranch}>
                <SelectTrigger className="h-8 text-sm">
                  <SelectValue placeholder="Select base branch" />
                </SelectTrigger>
                <SelectContent>
                  {localBranches.map((b) => (
                    <SelectItem key={b} value={b}>
                      {b}{b === currentBranch ? " (current)" : ""}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </TabsContent>

          {/* ── Existing Branch ── */}
          <TabsContent value="existing" className="px-4 py-3 space-y-3 min-h-0 overflow-y-auto">
            <Input
              placeholder="Search branches..."
              value={branchSearch}
              onChange={(e) => setBranchSearch(e.target.value)}
              className="h-8 text-sm"
            />
            <div className="space-y-0.5">
              {filteredBranches.map((branch) => {
                const hasWs = branchWorkspaceMap.has(branch);
                const isSelected = selectedBranch === branch;
                return (
                  <Button
                    key={branch}
                    variant="ghost"
                    className={cn(
                      "w-full justify-start gap-2 px-2 py-1.5 h-auto text-sm text-left min-w-0",
                      isSelected && "bg-accent",
                    )}
                    onClick={() => setSelectedBranch(branch)}
                    disabled={creating}
                  >
                    <GitBranch className="h-3 w-3 shrink-0 text-muted-foreground" />
                    <span className="truncate flex-1 min-w-0" title={branch}>{branch}</span>
                    <div className="flex gap-1 shrink-0">
                      {branch === currentBranch && (
                        <Badge variant="secondary" className="text-[10px]">
                          current
                        </Badge>
                      )}
                      {hasWs && (
                        <Badge variant="outline" className="text-[10px]">
                          open
                        </Badge>
                      )}
                    </div>
                  </Button>
                );
              })}
              {filteredBranches.length === 0 && (
                <p className="text-xs text-muted-foreground text-center py-4">
                  No branches found
                </p>
              )}
            </div>
          </TabsContent>

          {/* ── Pull Requests ── */}
          <TabsContent value="pr" className="px-4 py-3 space-y-3 min-h-0 overflow-y-auto">
            {!ghAvailable ? (
              <div className="text-center py-6 space-y-2">
                <AlertCircle className="h-8 w-8 mx-auto text-muted-foreground/40" />
                <p className="text-xs text-muted-foreground">
                  GitHub CLI not available or not a GitHub repo
                </p>
              </div>
            ) : prLoading ? (
              <div className="flex items-center justify-center py-6">
                <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
              </div>
            ) : (
              <div className="space-y-0.5">
                {prs.map((pr) => (
                  <Button
                    key={pr.number}
                    variant="ghost"
                    className="w-full justify-start gap-2 px-2 py-2 h-auto text-left items-start"
                    onClick={() =>
                      pr.head_branch &&
                      handleCreate(pr.head_branch, false)
                    }
                    disabled={creating || !pr.head_branch}
                  >
                    <GitPullRequest className="h-3.5 w-3.5 shrink-0 text-muted-foreground mt-0.5" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5">
                        <span className="text-sm truncate">
                          {pr.title}
                        </span>
                        <span className="text-[10px] text-muted-foreground shrink-0">
                          #{pr.number}
                        </span>
                      </div>
                      <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                        <GitBranch className="h-2.5 w-2.5" />
                        <span className="truncate">
                          {pr.head_branch}
                        </span>
                        {pr.is_draft && (
                          <Badge
                            variant="secondary"
                            className="text-[10px]"
                          >
                            draft
                          </Badge>
                        )}
                      </div>
                    </div>
                  </Button>
                ))}
                {prs.length === 0 && (
                  <p className="text-xs text-muted-foreground text-center py-4">
                    No open pull requests
                  </p>
                )}
              </div>
            )}
          </TabsContent>
        </Tabs>

        {/* ── Shared footer: preset + action button ── */}
        <div className="shrink-0 px-4 pb-4 pt-3 space-y-3">
          <PresetSelector
            presets={presets}
            selected={selectedPreset}
            onSelect={setSelectedPreset}
          />
          {activeTab !== "pr" && <Separator />}
          {activeTab === "new-branch" && (
            <Button
              className="w-full h-8 text-xs"
              disabled={!newBranchName.trim() || creating}
              onClick={() => handleCreate(newBranchName.trim(), true)}
            >
              {creating && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              Create Workspace
            </Button>
          )}
          {activeTab === "existing" && (
            <Button
              className="w-full h-8 text-xs"
              disabled={!selectedBranch || creating}
              onClick={() => {
                if (!selectedBranch) return;
                const existingWsId = branchWorkspaceMap.get(selectedBranch);
                if (existingWsId) {
                  activateWorkspace(existingWsId).then(() => onOpenChange(false));
                } else {
                  handleCreate(selectedBranch, false);
                }
              }}
            >
              {creating && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              Open Workspace
            </Button>
          )}
        </div>
        </>
        )}

        {error && (
          <div className="mx-4 mb-3 p-2 rounded-md bg-destructive/10 border border-destructive/20 text-xs text-destructive">
            {error}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
