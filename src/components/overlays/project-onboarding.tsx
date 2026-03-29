import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  GitBranch,
  ChevronRight,
  ChevronLeft,
  ChevronDown,
  Check,
} from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { PresetIcon } from "@/components/icons/preset-icon";
import {
  listBranches,
  listWorktrees,
  getDefaultBranch,
  generateBranchName,
  generateRandomBranchName,
  createWorktreeWorkspace,
  importWorktreeWorkspace,
  activateWorkspace,
  closeWorkspace,
  detectPackageManager,
  setProjectScripts,
  dbAddRecentProject,
  getPresets,
} from "@/tauri/commands";
import type { WorktreeInfo, DetectedSetup, TerminalPreset } from "@/tauri/types";

type Step = "workspace" | "setup";
type SetupMode = "checklist" | "custom";

interface Props {
  projectDir: string;
  tempWorkspaceId: string;
  onComplete: () => void;
  onCancel: () => void;
}

export function ProjectOnboarding({ projectDir, tempWorkspaceId, onComplete, onCancel: _onCancel }: Props) {
  // ── Step state ──
  const [step, setStep] = useState<Step>("workspace");

  // ── Step 1 state ──
  const [task, setTask] = useState("");
  const [generatedBranch, setGeneratedBranch] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");
  const [advancedOpen, setAdvancedOpen] = useState(false);

  // ── Step 2 state ──
  const [setupMode, setSetupMode] = useState<SetupMode>("checklist");
  const [actions, setActions] = useState<(DetectedSetup & { checked: boolean })[]>([]);
  const [setupContent, setSetupContent] = useState("");
  const [teardownContent, setTeardownContent] = useState("");
  const [teardownOpen, setTeardownOpen] = useState(false);

  // ── Agent state ──
  const [presets, setPresets] = useState<TerminalPreset[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const lastSelectedAgentId = useUIStore((s) => s.lastSelectedAgentId);
  const setLastSelectedAgentId = useUIStore((s) => s.setLastSelectedAgentId);
  const addPendingWorkspace = useUIStore((s) => s.addPendingWorkspace);
  const removePendingWorkspace = useUIStore((s) => s.removePendingWorkspace);
  const failPendingWorkspace = useUIStore((s) => s.failPendingWorkspace);

  // ── Data state ──
  const [allBranches, setAllBranches] = useState<string[]>([]);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [isCreating, setIsCreating] = useState(false);
  const [showImportConfirm, setShowImportConfirm] = useState(false);

  const taskInputRef = useRef<HTMLInputElement>(null);
  const branchGenTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── External worktrees (not the main repo, not bare, not detached) ──
  const externalWorktrees = useMemo(
    () =>
      worktrees.filter(
        (wt) =>
          wt.path !== projectDir &&
          !wt.is_bare &&
          wt.branch !== null &&
          wt.branch !== undefined,
      ),
    [worktrees, projectDir],
  );

  // ── Selected agent preset ──
  const selectedAgent = useMemo(
    () => presets.find((p) => p.id === selectedAgentId) ?? null,
    [presets, selectedAgentId],
  );

  // ── Load data on mount ──
  useEffect(() => {
    let cancelled = false;

    Promise.all([
      listBranches(projectDir, false).catch(() => []),
      listBranches(projectDir, true).catch(() => []),
      listWorktrees(projectDir).catch(() => []),
      getDefaultBranch(projectDir).catch(() => "main"),
      detectPackageManager(projectDir).catch(() => []),
      getPresets().catch(() => ({ presets: [], bar_visible: false, default_preset_id: null })),
    ]).then(([local, remote, wt, defBranch, detected, presetSnap]) => {
      if (cancelled) return;
      const branches = Array.from(
        new Set([...local, ...remote.map((b) => b.replace(/^origin\//, ""))]),
      ).sort();
      setAllBranches(branches);
      setWorktrees(wt);
      setBaseBranch(defBranch);
      setActions(detected.map((d) => ({ ...d, checked: d.enabled })));
      const pinned = presetSnap.presets.filter((p) => p.pinned);
      setPresets(pinned);
      setSelectedAgentId(lastSelectedAgentId || pinned.find((p) => p.id === "builtin-claude")?.id || pinned[0]?.id || null);
    });

    return () => {
      cancelled = true;
    };
  }, [projectDir, lastSelectedAgentId]);

  // ── Auto-focus task input ──
  useEffect(() => {
    if (step === "workspace") {
      setTimeout(() => taskInputRef.current?.focus(), 100);
    }
  }, [step]);

  // ── Debounced branch name generation ──
  const handleTaskChange = (value: string) => {
    setTask(value);
    if (branchGenTimeout.current) clearTimeout(branchGenTimeout.current);
    if (!value.trim()) {
      setGeneratedBranch("");
      return;
    }
    branchGenTimeout.current = setTimeout(async () => {
      try {
        const name = await generateBranchName(value, projectDir);
        setGeneratedBranch(name);
      } catch {
        setGeneratedBranch("");
      }
    }, 500);
  };

  // ── Step 1 → Step 2 ──
  const handleContinue = () => {
    if (!task.trim()) return;
    setStep("setup");
  };

  // ── Toggle action in checklist ──
  const toggleAction = (id: string) => {
    setActions((prev) =>
      prev.map((a) => (a.id === id ? { ...a, checked: !a.checked } : a)),
    );
  };

  // ── Switch to custom mode, pre-fill with checked commands ──
  const switchToCustom = () => {
    const checked = actions.filter((a) => a.checked).map((a) => a.command);
    setSetupContent(checked.join("\n"));
    setSetupMode("custom");
  };

  // ── Collect final setup commands ──
  const collectSetupCommands = (): string[] => {
    if (setupMode === "checklist") {
      return actions.filter((a) => a.checked).map((a) => a.command);
    }
    return setupContent
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
  };

  const collectTeardownCommands = (): string[] => {
    return teardownContent
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
  };

  // ── Create workspace ──
  const handleCreateWorkspace = useCallback(
    async (saveScripts: boolean) => {
      if (isCreating) return;
      setIsCreating(true);

      const tempId = crypto.randomUUID();
      const displayName = task.slice(0, 40) || "New workspace";

      addPendingWorkspace({
        id: tempId,
        name: displayName,
        projectPath: projectDir,
        status: "creating",
      });

      try {
        // Save scripts if requested
        if (saveScripts) {
          const setup = collectSetupCommands();
          const teardown = collectTeardownCommands();
          if (setup.length > 0 || teardown.length > 0) {
            await setProjectScripts(projectDir, {
              setup,
              teardown,
              run: null,
            });
          }
        }

        // Generate branch name if not already generated
        let branch = generatedBranch;
        if (!branch) {
          branch = task.trim()
            ? await generateBranchName(task, projectDir)
            : await generateRandomBranchName(projectDir);
        }

        // Close the temporary empty workspace before creating the real one
        await closeWorkspace(tempWorkspaceId, false).catch(() => {});

        const wsId = await createWorktreeWorkspace(
          projectDir,
          branch,
          true, // new branch
          "single",
          baseBranch || null,
          task.trim() || null,
          selectedAgentId,
        );

        const pName = projectDir.split("/").filter(Boolean).pop() || projectDir;
        dbAddRecentProject(projectDir, pName).catch(console.error);

        removePendingWorkspace(tempId);
        await activateWorkspace(wsId);
        onComplete();
      } catch (err) {
        failPendingWorkspace(tempId, String(err));
        setTimeout(() => removePendingWorkspace(tempId), 5000);
        setIsCreating(false);
      }
    },
    [
      isCreating,
      task,
      generatedBranch,
      baseBranch,
      selectedAgentId,
      projectDir,
      tempWorkspaceId,
      setupMode,
      actions,
      setupContent,
      teardownContent,
      onComplete,
      addPendingWorkspace,
      removePendingWorkspace,
      failPendingWorkspace,
    ],
  );

  // ── Import all external worktrees ──
  const handleImportAll = async () => {
    setShowImportConfirm(false);
    // Close the temporary empty workspace
    await closeWorkspace(tempWorkspaceId, false).catch(() => {});
    let lastWsId: string | null = null;
    for (const wt of externalWorktrees) {
      if (!wt.branch) continue;
      const branch = wt.branch.replace(/^refs\/heads\//, "");
      try {
        lastWsId = await importWorktreeWorkspace(wt.path, branch, "single");
      } catch (err) {
        console.error("Failed to import worktree:", wt.path, err);
      }
    }
    if (lastWsId) await activateWorkspace(lastWsId);
    const pName = projectDir.split("/").filter(Boolean).pop() || projectDir;
    dbAddRecentProject(projectDir, pName).catch(console.error);
    onComplete();
  };

  // ── Import single worktree ──
  const handleImportSingle = async (wt: WorktreeInfo) => {
    if (!wt.branch) return;
    const branch = wt.branch.replace(/^refs\/heads\//, "");
    try {
      // Close the temporary empty workspace
      await closeWorkspace(tempWorkspaceId, false).catch(() => {});
      const wsId = await importWorktreeWorkspace(wt.path, branch, "single");
      const pName = projectDir.split("/").filter(Boolean).pop() || projectDir;
      dbAddRecentProject(projectDir, pName).catch(console.error);
      await activateWorkspace(wsId);
      onComplete();
    } catch (err) {
      console.error("Failed to import worktree:", wt.path, err);
    }
  };

  // ── Key handling ──
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (step === "workspace") handleContinue();
    }
  };

  const projectName = projectDir.split("/").filter(Boolean).pop() || "project";

  return (
    <div className="flex-1 h-full flex flex-col overflow-hidden bg-background">
      {/* ── External worktrees banner — outside scroll container ── */}
      {externalWorktrees.length > 0 && (
        <div className="mx-6 mt-6 rounded-lg border border-border/60 bg-card/50 p-4">
          <div className="flex items-start justify-between gap-4">
            <div className="space-y-2 min-w-0">
              <p className="text-sm font-medium text-foreground">
                {externalWorktrees.length} existing worktree
                {externalWorktrees.length !== 1 ? "s" : ""} found
              </p>
              <div className="flex flex-wrap gap-1.5">
                {externalWorktrees.slice(0, 5).map((wt) => {
                  const branch = wt.branch?.replace(/^refs\/heads\//, "") ?? "";
                  return (
                    <button
                      key={wt.path}
                      type="button"
                      onClick={() => handleImportSingle(wt)}
                      className="inline-flex items-center gap-1 rounded-md bg-muted px-2 py-0.5 text-xs font-mono text-muted-foreground hover:bg-muted/80 hover:text-foreground transition-colors max-w-[180px]"
                    >
                      <GitBranch className="h-3 w-3 shrink-0" />
                      <span className="truncate">{branch}</span>
                    </button>
                  );
                })}
                {externalWorktrees.length > 5 && (
                  <span className="inline-flex items-center rounded-md bg-muted px-2 py-0.5 text-xs text-muted-foreground">
                    +{externalWorktrees.length - 5} more
                  </span>
                )}
              </div>
            </div>
            <Button
              variant="outline"
              size="sm"
              className="shrink-0"
              onClick={() => setShowImportConfirm(true)}
            >
              Import all
            </Button>
          </div>
        </div>
      )}

      {/* ── Scrollable content area ── */}
      <div className="flex-1 flex overflow-y-auto">
        <div className="flex-1 flex items-center justify-center px-6 py-8">
          <div className="w-full max-w-3xl space-y-6">
            {/* ── Header ── */}
            <div className="space-y-1.5">
              <p className="text-xs uppercase tracking-wide text-muted-foreground">
                Step {step === "workspace" ? 1 : 2} of 2
              </p>
              <h1 className="text-2xl font-semibold text-foreground">
                {step === "workspace" && "Create your first workspace"}
                {step === "setup" && "Setup script"}
              </h1>
              <p className="text-sm text-muted-foreground">
                {step === "workspace" &&
                  "Workspaces are isolated task environments backed by git worktrees."}
                {step === "setup" &&
                  "These commands run automatically when a workspace is created."}
              </p>
            </div>

            {/* ── Step 1: Workspace ── */}
            {step === "workspace" && (
              <div className="space-y-4" onKeyDown={handleKeyDown}>
                <div className="space-y-2">
                  <Label htmlFor="onboarding-task">Task</Label>
                  <Input
                    ref={taskInputRef}
                    id="onboarding-task"
                    className="h-11"
                    value={task}
                    onChange={(e) => handleTaskChange(e.target.value)}
                    placeholder="e.g. Add dark mode, Fix checkout bug"
                  />
                </div>

                {/* Branch name display — always visible */}
                <div className="rounded-md border border-border/60 bg-card/40 px-3 py-2 text-sm">
                  <div className="flex items-center gap-2 text-muted-foreground">
                    <GitBranch className="h-3.5 w-3.5 shrink-0" />
                    <span className="font-mono">
                      {generatedBranch || "branch-name"}
                    </span>
                    <span className="text-muted-foreground/50">from</span>
                    <span className="font-mono">{baseBranch}</span>
                  </div>
                </div>

                {/* Advanced options */}
                <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
                  <CollapsibleTrigger className="flex items-center gap-1.5 text-xs text-muted-foreground/80 hover:text-muted-foreground transition-colors py-1">
                    <ChevronDown
                      className={cn(
                        "h-3 w-3 transition-transform duration-200",
                        !advancedOpen && "-rotate-90",
                      )}
                    />
                    Advanced options
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <div className="pt-3 space-y-2">
                      <span className="text-xs font-medium text-muted-foreground">
                        Base branch
                      </span>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="outline"
                            className="w-full h-10 justify-between font-normal"
                          >
                            <span className="flex items-center gap-2 truncate">
                              <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                              <span className="truncate font-mono text-sm">
                                {baseBranch}
                              </span>
                              {(baseBranch === "main" || baseBranch === "master") && (
                                <span className="text-[10px] text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                                  default
                                </span>
                              )}
                            </span>
                            <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent className="max-h-[240px] w-[var(--radix-dropdown-menu-trigger-width)] overflow-y-auto">
                          {allBranches.map((branch) => (
                            <DropdownMenuItem
                              key={branch}
                              onClick={() => setBaseBranch(branch)}
                              className={cn(
                                "text-xs gap-2",
                                baseBranch === branch && "bg-accent",
                              )}
                            >
                              <span className="truncate flex-1">{branch}</span>
                              {(branch === "main" || branch === "master") && (
                                <span className="text-[10px] text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                                  default
                                </span>
                              )}
                            </DropdownMenuItem>
                          ))}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </CollapsibleContent>
                </Collapsible>

                {/* Continue button */}
                <div className="flex justify-end">
                  <Button
                    onClick={handleContinue}
                    disabled={!task.trim()}
                  >
                    Continue
                    <ChevronRight className="h-4 w-4" />
                  </Button>
                </div>
              </div>
            )}

            {/* ── Step 2: Setup ── */}
            {step === "setup" && (
              <div className="space-y-4">
                {/* Mode A: Checklist */}
                {setupMode === "checklist" && actions.length > 0 && (
                  <div className="space-y-3">
                    <div className="overflow-hidden rounded-lg border bg-card/40 divide-y divide-border/60">
                      {actions.map((action) => (
                        <button
                          key={action.id}
                          type="button"
                          onClick={() => toggleAction(action.id)}
                          className="flex items-center gap-3 w-full px-3 py-2.5 text-left hover:bg-muted/40 transition-colors cursor-pointer"
                        >
                          <div
                            className={cn(
                              "h-4 w-4 rounded border shrink-0 flex items-center justify-center transition-colors",
                              action.checked
                                ? "bg-primary border-primary"
                                : "border-border",
                            )}
                          >
                            {action.checked && (
                              <Check className="h-3 w-3 text-primary-foreground" />
                            )}
                          </div>
                          <div className="flex flex-col min-w-0">
                            <span className="text-sm text-foreground">
                              {action.label}
                            </span>
                            <span className="text-xs text-muted-foreground font-mono truncate">
                              {action.command}
                            </span>
                          </div>
                        </button>
                      ))}
                    </div>
                    <button
                      type="button"
                      onClick={switchToCustom}
                      className="text-xs text-muted-foreground hover:text-foreground underline underline-offset-2"
                    >
                      Customize commands
                    </button>
                  </div>
                )}

                {/* Mode B: No detection */}
                {setupMode === "checklist" && actions.length === 0 && (
                  <div className="overflow-hidden rounded-lg border bg-card/40 p-6 text-center space-y-3">
                    <p className="text-sm text-muted-foreground">
                      We couldn't detect a package manager or environment config.
                    </p>
                    <div className="flex items-center justify-center gap-2">
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => setSetupMode("custom")}
                      >
                        Add commands
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => handleCreateWorkspace(false)}
                        disabled={isCreating}
                      >
                        Skip
                      </Button>
                    </div>
                  </div>
                )}

                {/* Mode C: Custom commands */}
                {setupMode === "custom" && (
                  <div className="space-y-3">
                    {actions.length > 0 && (
                      <button
                        type="button"
                        onClick={() => setSetupMode("checklist")}
                        className="text-xs text-muted-foreground hover:text-foreground underline underline-offset-2"
                      >
                        Back to checklist
                      </button>
                    )}
                    <div className="overflow-hidden rounded-lg border bg-card/40">
                      <div className="p-3 space-y-3">
                        <Textarea
                          value={setupContent}
                          onChange={(e) => setSetupContent(e.target.value)}
                          placeholder="Add setup commands, one per line..."
                          className="h-full min-h-[220px] resize-none overflow-x-auto whitespace-pre font-mono text-xs"
                        />
                        <div className="flex flex-wrap items-center gap-1.5 border-t px-1 pt-2 text-[11px] text-muted-foreground">
                          <span className="mr-1">Variables</span>
                          <span className="rounded bg-muted px-1.5 py-0.5 font-mono">
                            $CODEMUX_ROOT_PATH
                          </span>
                          <span className="rounded bg-muted px-1.5 py-0.5 font-mono">
                            $CODEMUX_WORKSPACE_PATH
                          </span>
                        </div>
                      </div>
                    </div>
                  </div>
                )}

                {/* Teardown commands */}
                <Collapsible
                  open={teardownOpen}
                  onOpenChange={setTeardownOpen}
                >
                  <CollapsibleTrigger className="flex items-center gap-1.5 text-xs text-muted-foreground/80 hover:text-muted-foreground transition-colors py-1">
                    <ChevronDown
                      className={cn(
                        "h-3 w-3 transition-transform duration-200",
                        !teardownOpen && "-rotate-90",
                      )}
                    />
                    Teardown commands (optional)
                  </CollapsibleTrigger>
                  <CollapsibleContent className="pt-2">
                    <Textarea
                      value={teardownContent}
                      onChange={(e) => setTeardownContent(e.target.value)}
                      placeholder="docker compose down"
                      className="min-h-20 font-mono text-xs"
                    />
                  </CollapsibleContent>
                </Collapsible>

                {/* Buttons */}
                <div className="flex justify-between">
                  <Button
                    variant="outline"
                    onClick={() => setStep("workspace")}
                  >
                    <ChevronLeft className="h-4 w-4" />
                    Back
                  </Button>
                  <div className="flex items-center gap-2">
                    {/* Agent picker */}
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <button
                          type="button"
                          className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-[11px] text-foreground transition-colors outline-none hover:bg-muted"
                        >
                          {selectedAgent ? (
                            <>
                              <PresetIcon
                                icon={selectedAgent.icon}
                                className="h-3.5 w-3.5"
                              />
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
                      <DropdownMenuContent align="end" className="w-[200px]">
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

                    <Button
                      variant="outline"
                      onClick={() => handleCreateWorkspace(false)}
                      disabled={isCreating}
                    >
                      Skip for now
                    </Button>
                    <Button
                      onClick={() => handleCreateWorkspace(true)}
                      disabled={isCreating}
                    >
                      {isCreating ? "Creating..." : "Create workspace"}
                      <ChevronRight className="h-4 w-4" />
                    </Button>
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* ── Import confirmation dialog ── */}
      <AlertDialog open={showImportConfirm} onOpenChange={setShowImportConfirm}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Import all worktrees</AlertDialogTitle>
            <AlertDialogDescription>
              This will import {externalWorktrees.length} existing worktree
              {externalWorktrees.length !== 1 ? "s" : ""} into {projectName} as
              workspaces. Each worktree on disk will be tracked and appear in
              your sidebar. No files will be modified.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={handleImportAll}>
              Import all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
