import { useState, useEffect, useMemo, useCallback } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import {
  GitBranch,
  GitPullRequest,
  Plus,
  Loader2,
  AlertCircle,
} from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import {
  listBranches,
  listWorktrees,
  getGitBranchInfo,
  createWorktreeWorkspace,
  importWorktreeWorkspace,
  activateWorkspace,
  getPresets,
  applyPreset,
  checkGhAvailable,
  checkGithubRepo,
  listPullRequests,
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
          <button
            key={p.id}
            className={`px-2.5 py-1 text-xs rounded-md border transition-colors ${
              selected === p.id
                ? "border-primary bg-primary/10 text-foreground"
                : "border-border bg-card text-muted-foreground hover:border-muted-foreground"
            }`}
            onClick={() => onSelect(selected === p.id ? null : p.id)}
          >
            {p.name}
          </button>
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
  const cwd = activeWs?.cwd ?? "";

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
  const [newBranchName, setNewBranchName] = useState("");
  const [baseBranch, setBaseBranch] = useState("");

  // PR state
  const [prs, setPrs] = useState<PullRequestInfo[]>([]);
  const [ghAvailable, setGhAvailable] = useState(false);
  const [prLoading, setPrLoading] = useState(false);

  // Load data when dialog opens
  useEffect(() => {
    if (!open || !cwd) return;
    setError(null);
    setCreating(false);
    setNewBranchName("");
    setBranchSearch("");

    // Fetch branches
    Promise.all([
      listBranches(cwd, false).catch(() => []),
      listBranches(cwd, true).catch(() => []),
      listWorktrees(cwd).catch(() => []),
      getGitBranchInfo(cwd).catch(() => ({ branch: null, ahead: 0, behind: 0 })),
    ]).then(([local, remote, wt, info]) => {
      setLocalBranches(local);
      setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
      setWorktrees(wt);
      setCurrentBranch(info.branch);
      setBaseBranch(info.branch ?? "main");
    });

    // Fetch presets
    getPresets()
      .then((snap) => setPresets(snap.presets.filter((p) => p.pinned)))
      .catch(() => {});

    // Check gh
    Promise.all([checkGhAvailable(), checkGithubRepo(cwd)])
      .then(([available, isRepo]) => {
        setGhAvailable(available && isRepo);
        if (available && isRepo) {
          setPrLoading(true);
          listPullRequests(cwd, "open")
            .then(setPrs)
            .catch(() => setPrs([]))
            .finally(() => setPrLoading(false));
        }
      })
      .catch(() => setGhAvailable(false));
  }, [open, cwd]);

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

  // Check if branch already has a workspace
  const branchWorkspaceMap = useMemo(() => {
    const map = new Map<string, string>();
    if (appState) {
      for (const ws of appState.workspaces) {
        if (ws.git_branch) map.set(ws.git_branch, ws.workspace_id);
      }
    }
    return map;
  }, [appState]);

  const handleCreate = useCallback(
    async (branch: string, createBranch: boolean) => {
      if (!cwd || creating) return;
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

        // Check for orphan worktree
        const orphan = worktrees.find(
          (wt) => wt.branch === branch || wt.branch === `refs/heads/${branch}`,
        );

        let wsId: string;
        if (orphan) {
          wsId = await importWorktreeWorkspace(orphan.path, cwd);
        } else {
          wsId = await createWorktreeWorkspace(
            branch,
            "", // path - let backend decide
            cwd,
            createBranch,
          );
        }

        // Apply preset if selected
        if (selectedPreset) {
          await applyPreset(wsId, selectedPreset, "split_pane").catch(
            console.error,
          );
        }

        onOpenChange(false);
      } catch (err) {
        setError(String(err));
      } finally {
        setCreating(false);
      }
    },
    [cwd, creating, selectedPreset, branchWorkspaceMap, worktrees, onOpenChange],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[500px] p-0 gap-0">
        <DialogHeader className="p-4 pb-0">
          <DialogTitle className="text-sm">New Workspace</DialogTitle>
        </DialogHeader>

        <Tabs defaultValue="new-branch" className="flex flex-col">
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
          <TabsContent value="new-branch" className="p-4 pt-3 space-y-3">
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
              <select
                className="flex h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                value={baseBranch}
                onChange={(e) => setBaseBranch(e.target.value)}
              >
                {localBranches.map((b) => (
                  <option key={b} value={b}>
                    {b}
                    {b === currentBranch ? " (current)" : ""}
                  </option>
                ))}
              </select>
            </div>
            <PresetSelector
              presets={presets}
              selected={selectedPreset}
              onSelect={setSelectedPreset}
            />
            <Separator />
            <Button
              className="w-full h-8 text-xs"
              disabled={!newBranchName.trim() || creating}
              onClick={() => handleCreate(newBranchName.trim(), true)}
            >
              {creating && <Loader2 className="h-3 w-3 animate-spin mr-1" />}
              Create Workspace
            </Button>
          </TabsContent>

          {/* ── Existing Branch ── */}
          <TabsContent value="existing" className="p-4 pt-3 space-y-3">
            <Input
              placeholder="Search branches..."
              value={branchSearch}
              onChange={(e) => setBranchSearch(e.target.value)}
              className="h-8 text-sm"
            />
            <ScrollArea className="h-48">
              <div className="space-y-0.5">
                {filteredBranches.map((branch) => {
                  const hasWs = branchWorkspaceMap.has(branch);
                  return (
                    <button
                      key={branch}
                      className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm text-left hover:bg-accent"
                      onClick={() =>
                        hasWs
                          ? activateWorkspace(
                              branchWorkspaceMap.get(branch)!,
                            ).then(() => onOpenChange(false))
                          : handleCreate(branch, false)
                      }
                      disabled={creating}
                    >
                      <GitBranch className="h-3 w-3 shrink-0 text-muted-foreground" />
                      <span className="truncate flex-1">{branch}</span>
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
                    </button>
                  );
                })}
                {filteredBranches.length === 0 && (
                  <p className="text-xs text-muted-foreground text-center py-4">
                    No branches found
                  </p>
                )}
              </div>
            </ScrollArea>
            <PresetSelector
              presets={presets}
              selected={selectedPreset}
              onSelect={setSelectedPreset}
            />
          </TabsContent>

          {/* ── Pull Requests ── */}
          <TabsContent value="pr" className="p-4 pt-3 space-y-3">
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
              <ScrollArea className="h-56">
                <div className="space-y-0.5">
                  {prs.map((pr) => (
                    <button
                      key={pr.number}
                      className="flex w-full items-start gap-2 rounded-md px-2 py-2 text-left hover:bg-accent"
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
                    </button>
                  ))}
                  {prs.length === 0 && (
                    <p className="text-xs text-muted-foreground text-center py-4">
                      No open pull requests
                    </p>
                  )}
                </div>
              </ScrollArea>
            )}
            <PresetSelector
              presets={presets}
              selected={selectedPreset}
              onSelect={setSelectedPreset}
            />
          </TabsContent>
        </Tabs>

        {error && (
          <div className="mx-4 mb-3 p-2 rounded-md bg-destructive/10 border border-destructive/20 text-xs text-destructive">
            {error}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
