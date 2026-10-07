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
  X,
} from "lucide-react";
import { basename } from "@/lib/path";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { BranchPicker } from "./branch-picker";
import { useUIStore } from "@/stores/ui-store";
import {
  listBranchesDetailed,
  listWorktrees,
  getDefaultBranch,
  generateBranchName,
  generateRandomBranchName,
  createWorktreeWorkspaceResult,
  importWorktreeWorkspace,
  activateWorkspace,
  closeWorkspace,
  detectPackageManager,
  setProjectScripts,
  dbAddRecentProject,
  getPresets,
} from "@/tauri/commands";
import type {
  WorktreeInfo,
  DetectedSetup,
  BranchDetail,
  TerminalPreset,
} from "@/tauri/types";
import { randomUUID } from "@/lib/uuid";
import { toast } from "@/lib/toast";
import { Eyebrow } from "@/components/ui/eyebrow";
import { PresetIcon } from "@/components/icons/preset-icon";

type Step = "workspace" | "setup";
type SetupMode = "checklist" | "custom";

interface Props {
  projectDir: string;
  tempWorkspaceId: string;
  onComplete: () => void;
  onCancel: () => void;
}

export function ProjectOnboarding({ projectDir, tempWorkspaceId, onComplete, onCancel }: Props) {
  // ── Step state ──
  const [step, setStep] = useState<Step>("workspace");

  // ── Step 1 state ──
  const [task, setTask] = useState("");
  const [generatedBranch, setGeneratedBranch] = useState("");
  const [branchEdited, setBranchEdited] = useState(false);
  const [baseBranch, setBaseBranch] = useState("main");

  // ── Step 2 state ──
  const [setupMode, setSetupMode] = useState<SetupMode>("checklist");
  const [actions, setActions] = useState<(DetectedSetup & { checked: boolean })[]>([]);
  const [setupContent, setSetupContent] = useState("");
  const [teardownContent, setTeardownContent] = useState("");
  const [teardownOpen, setTeardownOpen] = useState(false);

  // ── Agent state ──
  const [agents, setAgents] = useState<TerminalPreset[]>([]);
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const setLastSelectedAgentId = useUIStore((s) => s.setLastSelectedAgentId);
  const addPendingWorkspace = useUIStore((s) => s.addPendingWorkspace);
  const removePendingWorkspace = useUIStore((s) => s.removePendingWorkspace);
  const failPendingWorkspace = useUIStore((s) => s.failPendingWorkspace);

  // ── Data state ──
  const [detailedBranches, setDetailedBranches] = useState<BranchDetail[]>([]);
  const [branchesLoading, setBranchesLoading] = useState(true);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [isCreating, setIsCreating] = useState(false);
  const [showImportConfirm, setShowImportConfirm] = useState(false);
  const [importProgress, setImportProgress] = useState<{ current: number; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Which way the last step change went, so the entering step slides in
  // from the side the user is moving toward.
  const [stepDirection, setStepDirection] = useState<"forward" | "back">("forward");

  const taskInputRef = useRef<HTMLInputElement>(null);
  const setupStepRef = useRef<HTMLDivElement>(null);
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

  // ── Load data on mount ──
  useEffect(() => {
    let cancelled = false;
    setBranchesLoading(true);

    Promise.all([
      listBranchesDetailed(projectDir).catch(() => []),
      listWorktrees(projectDir).catch(() => []),
      getDefaultBranch(projectDir).catch(() => "main"),
      detectPackageManager(projectDir).catch(() => []),
      getPresets().catch(() => ({ presets: [], bar_visible: false, default_preset_id: null })),
    ]).then(([detailed, wt, defBranch, detected, presetSnap]) => {
      if (cancelled) return;
      setDetailedBranches(detailed);
      setBranchesLoading(false);
      setWorktrees(wt);
      setBaseBranch(defBranch);
      setActions(detected.map((d) => ({ ...d, checked: d.enabled })));
      // Same agent list as NewWorkspaceDialog: pinned CLI presets only.
      const cliAgents = presetSnap.presets.filter((p) => p.pinned && p.kind === "cli");
      // Read once rather than subscribing: picking an agent below updates
      // the store, and that must not re-run this whole mount load.
      const last = useUIStore.getState().lastSelectedAgentId;
      setAgents(cliAgents);
      setSelectedAgentId(
        cliAgents.find((p) => p.id === last)?.id ??
          cliAgents.find((p) => p.id === "builtin-claude")?.id ??
          cliAgents[0]?.id ??
          null,
      );
    });

    return () => {
      cancelled = true;
    };
  }, [projectDir]);

  // ── Focus handoff on step change ──
  // The control that moved the user (Continue / Back) unmounts with its
  // step, so focus would otherwise fall to <body>.
  useEffect(() => {
    const timer = setTimeout(() => {
      if (step === "workspace") {
        taskInputRef.current?.focus();
      } else {
        setupStepRef.current
          ?.querySelector<HTMLElement>("button, textarea")
          ?.focus();
      }
    }, 100);
    return () => clearTimeout(timer);
  }, [step]);

  const selectedAgent = useMemo(
    () => agents.find((p) => p.id === selectedAgentId) ?? null,
    [agents, selectedAgentId],
  );

  // ── Clear debounce timer on unmount to avoid post-unmount setState ──
  useEffect(() => {
    return () => {
      if (branchGenTimeout.current) clearTimeout(branchGenTimeout.current);
    };
  }, []);

  // ── Skip onboarding — dismiss wizard, leave temp workspace intact ──
  const handleSkip = () => {
    if (branchGenTimeout.current) clearTimeout(branchGenTimeout.current);
    onCancel();
  };

  // ── Debounced branch name generation (only when not manually edited) ──
  const handleTaskChange = (value: string) => {
    setTask(value);
    if (branchEdited) return;
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

  // ── Manual branch name edit ──
  const handleBranchChange = (value: string) => {
    setBranchEdited(true);
    setGeneratedBranch(value);
  };

  // ── Step 1 → Step 2 ──
  const handleContinue = () => {
    if (!task.trim()) return;
    setStepDirection("forward");
    setStep("setup");
  };

  const handleBack = () => {
    setStepDirection("back");
    setStep("workspace");
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
      setError(null);

      const tempId = randomUUID();
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
              worktree_includes: [],
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

        // The task is the agent's first prompt, so the workspace opens with
        // the agent already working on what the user just described.
        const prompt = selectedAgentId ? task.trim() || null : null;
        const created = await createWorktreeWorkspaceResult(
          projectDir,
          branch,
          true, // new branch
          "single",
          baseBranch || null,
          prompt,
          selectedAgentId,
        );
        // The backend reused a live workspace for this worktree and dropped
        // the prompt rather than type into a running session; say so.
        if (created.adopted && prompt) {
          toast.info(`"${branch}" already has a live workspace — switched to it. Your task wasn't sent.`);
        }

        const pName = basename(projectDir);
        dbAddRecentProject(projectDir, pName).catch(console.error);

        removePendingWorkspace(tempId);
        await activateWorkspace(created.workspaceId);
        // Retire the temporary root workspace only once the real one exists.
        // Closing it first unmounts this wizard, so a failed create used to
        // drop the user on the empty state with no error and no project.
        await closeWorkspace(tempWorkspaceId, false).catch(() => {});
        onComplete();
      } catch (err) {
        failPendingWorkspace(tempId, String(err));
        setTimeout(() => removePendingWorkspace(tempId), 5000);
        setError(`Couldn't create the workspace: ${String(err)}`);
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
    setError(null);
    const targets = externalWorktrees.flatMap((wt) =>
      wt.branch
        ? [{ path: wt.path, branch: wt.branch.replace(/^refs\/heads\//, "") }]
        : [],
    );
    const imported: string[] = [];
    const failed: { branch: string; error: string }[] = [];
    for (const [index, wt] of targets.entries()) {
      setImportProgress({ current: index + 1, total: targets.length });
      try {
        imported.push(await importWorktreeWorkspace(wt.path, wt.branch, "single"));
      } catch (err) {
        console.error("Failed to import worktree:", wt.path, err);
        failed.push({ branch: wt.branch, error: String(err) });
      }
    }
    setImportProgress(null);

    // Nothing imported: keep the temporary workspace (and this wizard) so
    // the project stays open and the user can see what went wrong.
    if (imported.length === 0) {
      setError(
        failed.length > 0
          ? `Couldn't import any worktrees: ${failed[0].error}`
          : "No worktrees to import.",
      );
      return;
    }

    if (failed.length > 0) {
      toast.warning(`${imported.length} imported, ${failed.length} failed`, {
        description: `Not imported: ${failed.map((f) => f.branch).join(", ")}`,
      });
    }
    await activateWorkspace(imported[imported.length - 1]).catch(console.error);
    await closeWorkspace(tempWorkspaceId, false).catch(() => {});
    const pName = basename(projectDir);
    dbAddRecentProject(projectDir, pName).catch(console.error);
    onComplete();
  };

  // ── Key handling ──
  // Bound to the Step 1 text inputs only: on the step wrapper it also caught
  // Enter from the agent menu and branch picker, which bubble through React
  // portals, and advanced the step instead of picking the item.
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleContinue();
    }
  };

  const projectName = basename(projectDir) || "project";
  const stepIndex = step === "workspace" ? 0 : 1;
  // motion-safe: reduced-motion users get the instant swap.
  const stepEnterClass = cn(
    "motion-safe:animate-in motion-safe:fade-in-0 motion-safe:duration-150",
    stepDirection === "forward"
      ? "motion-safe:slide-in-from-right-2"
      : "motion-safe:slide-in-from-left-2",
  );
  const createLabel = selectedAgent ? `Create & start ${selectedAgent.name}` : "Create workspace";

  const errorAlert = error && (
    <div
      role="alert"
      className="flex items-start gap-2 rounded-md border border-destructive/20 bg-destructive/10 px-4 py-3"
    >
      <span className="flex-1 text-body text-destructive break-words">{error}</span>
      <button
        type="button"
        onClick={() => setError(null)}
        aria-label="Dismiss error"
        className="shrink-0 rounded-sm p-0.5 text-destructive/70 hover:text-destructive transition-colors duration-150"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );

  return (
    <div className="relative flex-1 h-full flex flex-col overflow-hidden bg-background">
      {/* ── Skip affordance — top-right, always visible ── */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            onClick={handleSkip}
            aria-label="Skip onboarding"
            className="absolute top-4 right-4 z-10 text-muted-foreground hover:text-foreground"
          >
            <X className="size-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Skip (Esc)</TooltipContent>
      </Tooltip>

      {/* ── External worktrees banner — outside scroll container ── */}
      {externalWorktrees.length > 0 && (
        <div className="mx-6 mt-6 rounded-lg border border-border/60 bg-card/50 p-4">
          <div className="flex items-start justify-between gap-4">
            <div className="space-y-2 min-w-0">
              <p className="text-body font-medium text-foreground">
                {externalWorktrees.length} existing worktree
                {externalWorktrees.length !== 1 ? "s" : ""} found
              </p>
              <div className="flex flex-wrap gap-1.5">
                {externalWorktrees.slice(0, 5).map((wt) => {
                  const branch = wt.branch?.replace(/^refs\/heads\//, "") ?? "";
                  return (
                    <span
                      key={wt.path}
                      className="inline-flex items-center gap-1 rounded-md bg-muted px-2 py-0.5 text-label font-mono text-muted-foreground max-w-[180px]"
                    >
                      <GitBranch className="size-3 shrink-0" />
                      <span className="truncate">{branch}</span>
                    </span>
                  );
                })}
                {externalWorktrees.length > 5 && (
                  <span className="inline-flex items-center rounded-md bg-muted px-2 py-0.5 text-label text-muted-foreground">
                    +{externalWorktrees.length - 5} more
                  </span>
                )}
              </div>
            </div>
            <Button
              variant="outline"
              size="sm"
              className="shrink-0 tabular-nums"
              onClick={() => setShowImportConfirm(true)}
              disabled={importProgress !== null || isCreating}
            >
              {importProgress
                ? `Importing ${importProgress.current} / ${importProgress.total}…`
                : "Import all"}
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
              <div className="flex items-center gap-3">
                <div className="flex gap-1" aria-hidden="true">
                  {(["workspace", "setup"] as const).map((segment, index) => (
                    <span
                      key={segment}
                      data-testid="onboarding-step-segment"
                      data-active={index <= stepIndex || undefined}
                      className={cn(
                        "h-0.5 w-8 rounded-sm transition-colors duration-150",
                        index <= stepIndex ? "bg-foreground" : "bg-surface-3",
                      )}
                    />
                  ))}
                </div>
                <Eyebrow>Step {stepIndex + 1} of 2</Eyebrow>
              </div>
              <h1 className="text-2xl font-medium tracking-tight text-foreground">
                {step === "workspace" && "Create your first workspace"}
                {step === "setup" && "Setup script"}
              </h1>
              <p className="text-body text-muted-foreground">
                {step === "workspace" &&
                  "Workspaces are isolated task environments backed by git worktrees."}
                {step === "setup" &&
                  "These commands run automatically when a workspace is created."}
              </p>
            </div>

            {/* ── Step 1: Workspace ── */}
            {step === "workspace" && (
              <div
                key="workspace"
                className={cn("space-y-4", stepEnterClass)}
              >
                <div className="space-y-2">
                  <Label htmlFor="onboarding-task">Task</Label>
                  <Input
                    ref={taskInputRef}
                    id="onboarding-task"
                    className="h-11"
                    value={task}
                    onChange={(e) => handleTaskChange(e.target.value)}
                    onKeyDown={handleKeyDown}
                    placeholder="e.g. Add dark mode, Fix checkout bug"
                  />
                </div>

                {/* Branch name — editable, with inline base branch picker */}
                <div className="rounded-md border border-border/60 bg-card/40 px-3 py-1.5 text-body">
                  <div className="flex items-center gap-2 text-muted-foreground">
                    <GitBranch className="size-3.5 shrink-0" />
                    <input
                      type="text"
                      value={generatedBranch}
                      onChange={(e) => handleBranchChange(e.target.value)}
                      onKeyDown={handleKeyDown}
                      placeholder="branch-name"
                      className="flex-1 min-w-0 bg-transparent font-mono text-body text-muted-foreground placeholder:text-muted-foreground/40 outline-none"
                    />
                    <span className="text-muted-foreground/50 shrink-0">from</span>
                    <BranchPicker
                      baseBranch={baseBranch}
                      branches={detailedBranches}
                      loading={branchesLoading}
                      onSelectBase={setBaseBranch}
                    />
                  </div>
                </div>

                {errorAlert}

                {/* Agent that starts on the task, then Continue */}
                <div className="flex items-center justify-between gap-3">
                  {agents.length > 0 ? (
                    <div className="flex items-center gap-2 text-label text-muted-foreground">
                      <span>Agent</span>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <button
                            type="button"
                            aria-label={`Agent: ${selectedAgent?.name ?? "none"}`}
                            className="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface-1 px-2.5 py-1 text-label text-foreground transition-colors duration-100 hover:bg-surface-2"
                          >
                            {selectedAgent && (
                              <PresetIcon icon={selectedAgent.icon} className="size-3.5" />
                            )}
                            {selectedAgent?.name ?? "Choose agent"}
                            <ChevronDown className="size-3 opacity-40" />
                          </button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="start" className="w-[200px]">
                          {agents.map((p) => (
                            <DropdownMenuItem
                              key={p.id}
                              onClick={() => {
                                setSelectedAgentId(p.id);
                                setLastSelectedAgentId(p.id);
                              }}
                              className="text-label gap-2"
                            >
                              <PresetIcon icon={p.icon} className="size-3.5" />
                              <span className="flex-1">{p.name}</span>
                              {selectedAgentId === p.id && (
                                <Check className="size-3.5 text-primary" />
                              )}
                            </DropdownMenuItem>
                          ))}
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  ) : (
                    <span />
                  )}
                  <Button
                    onClick={handleContinue}
                    disabled={!task.trim()}
                    className="bg-foreground text-background hover:bg-foreground/90"
                  >
                    Continue
                    <ChevronRight className="size-4" />
                  </Button>
                </div>
              </div>
            )}

            {/* ── Step 2: Setup ── */}
            {step === "setup" && (
              <div
                key="setup"
                ref={setupStepRef}
                className={cn("space-y-4", stepEnterClass)}
              >
                {/* Mode A: Checklist */}
                {setupMode === "checklist" && actions.length > 0 && (
                  <div className="space-y-3">
                    <div className="overflow-hidden rounded-lg border bg-card/40 divide-y divide-border/60">
                      {actions.map((action) => (
                        <button
                          key={action.id}
                          type="button"
                          onClick={() => toggleAction(action.id)}
                          className="flex items-center gap-3 w-full px-3 py-2.5 text-left hover:bg-muted/40 transition-colors duration-150 cursor-pointer"
                        >
                          <div
                            className={cn(
                              "size-4 rounded-sm border shrink-0 flex items-center justify-center transition-colors duration-150",
                              action.checked
                                ? "bg-primary border-primary"
                                : "border-border",
                            )}
                          >
                            {action.checked && (
                              <Check className="size-3 text-primary-foreground" />
                            )}
                          </div>
                          <div className="flex flex-col min-w-0">
                            <span className="text-body text-foreground">
                              {action.label}
                            </span>
                            <span className="text-label text-muted-foreground font-mono truncate">
                              {action.command}
                            </span>
                          </div>
                        </button>
                      ))}
                    </div>
                    <button
                      type="button"
                      onClick={switchToCustom}
                      className="text-label text-muted-foreground hover:text-foreground underline underline-offset-2"
                    >
                      Customize commands
                    </button>
                  </div>
                )}

                {/* Mode B: No detection */}
                {setupMode === "checklist" && actions.length === 0 && (
                  <div className="overflow-hidden rounded-lg border bg-card/40 p-6 text-center space-y-3">
                    <p className="text-body text-muted-foreground">
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
                        className="text-label text-muted-foreground hover:text-foreground underline underline-offset-2"
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
                          className="h-full min-h-[220px] resize-none overflow-x-auto whitespace-pre font-mono text-label"
                        />
                        <div className="flex flex-wrap items-center gap-1.5 border-t px-1 pt-2 text-label text-muted-foreground">
                          <span className="mr-1">Variables</span>
                          <span className="rounded-sm bg-muted px-1.5 py-0.5 font-mono">
                            $CODEMUX_ROOT_PATH
                          </span>
                          <span className="rounded-sm bg-muted px-1.5 py-0.5 font-mono">
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
                  <CollapsibleTrigger className="flex items-center gap-1.5 text-label text-muted-foreground/80 hover:text-muted-foreground transition-colors duration-150 py-1">
                    <ChevronDown
                      className={cn(
                        "size-3 transition-transform duration-150",
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
                      className="min-h-20 font-mono text-label"
                    />
                  </CollapsibleContent>
                </Collapsible>

                {errorAlert}

                {/* Buttons */}
                <div className="flex justify-between">
                  <Button
                    variant="outline"
                    onClick={handleBack}
                    disabled={isCreating}
                  >
                    <ChevronLeft className="size-4" />
                    Back
                  </Button>
                  <div className="flex items-center gap-2">
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
                      className="bg-foreground text-background hover:bg-foreground/90"
                    >
                      {isCreating ? "Creating…" : createLabel}
                      <ChevronRight className="size-4" />
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
            <AlertDialogAction onClick={handleImportAll} className="bg-foreground text-background hover:bg-foreground/90">
              Import all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
