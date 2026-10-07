import { useMobileLayout } from "@/hooks/use-mobile-layout";
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
import { basename } from "@/lib/path";
import { BranchPicker } from "./branch-picker";
import { WorkspaceAttachmentChip } from "./workspace-attachment-chip";
import { DevicePicker } from "@/components/hosts/device-picker";
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
  FolderOpen,
  GitBranch,
} from "lucide-react";
import { toast } from "@/lib/toast";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { PresetIcon } from "@/components/icons/preset-icon";
import { ProjectPicker } from "./project-picker";
import { resolveProvider } from "@/lib/source-control";
import { IssuePickerPanel } from "@/components/github/issue-picker";
import { PrPickerPanel } from "@/components/github/pr-picker";
import { useDefaultBranch } from "@/components/layout/default-branch-cache";
import {
  listBranches,
  listBranchesDetailed,
  listWorktrees,
  getGitBranchInfo,
  gitFetchPrune,
  createWorkspace,
  createWorktreeWorkspaceResult,
  importWorktreeWorkspace,
  setWorkspaceHost,
  activateWorkspace,
  getPresets,
  checkIsGitRepo,
  dbAddRecentProject,
  generateBranchName,
  generateRandomBranchName,
  listPullRequests,
  pasteClipboardImageToFile,
  suggestIssueBranchName,
  linkWorkspaceIssue,
  getGithubIssueByPath,
  applyPreset,
  renameWorkspace,
} from "@/tauri/commands";
import { fetchProviderAuth } from "@/lib/provider-auth";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { pickFiles } from "@/lib/file-dialog";
import type { TerminalPreset, WorktreeInfo, BranchDetail, PullRequestInfo, GitHubIssue, LinkedIssue, ModelSelection, NewWorkspaceDraft } from "@/tauri/types";
import { LaunchModelPicker } from "./launch-model-picker";
import { LaunchReasoningPicker } from "./launch-reasoning-picker";
import {
  detectLaunchFamily,
  familyToProviderKind,
  GEMINI_MODELS,
  parseBakedModel,
  REASONING_FLAG_FAMILIES,
  type LaunchModel,
  type ReasoningOption,
} from "@/lib/launch-models";
import {
  refreshProviderCapabilitiesForIntent,
  selectProviderCapabilitiesLoaded,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import {
  useLaunchGeminiModels,
  useLaunchGeminiModelsInit,
} from "@/stores/gemini-models-store";
import { randomUUID } from "@/lib/uuid";

const ISSUE_BODY_MAX_CHARS = 10_000;

/** Build a prompt with issue context prepended. Exported for testing. */
export function buildPromptWithIssueContext(
  userPrompt: string,
  issue: Pick<LinkedIssue, "number" | "title" | "state" | "labels"> | null,
  issueBody: string | null,
  /** Hosting product the issue came from. Defaults to GitHub so callers
   *  that never learned about detection keep their exact prompt text. */
  providerName = "GitHub",
): string {
  if (!issue) return userPrompt;

  const lines: string[] = [
    `The following ${providerName} issue is linked to this workspace:`,
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

/** Apply the dialog's optional "Workspace name" to the workspace that was
 *  just created.
 *
 *  Every create path here lands on a backend-assigned title —
 *  `default_workspace_title` (the directory's own name) for the repo-root
 *  paths, the branch name for the worktree paths
 *  (`set_workspace_worktree`) — so a name the user typed has to be
 *  applied explicitly afterwards. Without this the input only ever
 *  labelled the optimistic pending row and was silently discarded the
 *  moment the real workspace landed.
 *
 *  A typed name is the strongest available signal, so it wins over the
 *  branch-derived title too. Best-effort: a rename failure leaves the
 *  backend title rather than failing a workspace that already exists.
 *
 *  Callers must skip this when the backend ADOPTED a pre-existing live
 *  workspace instead of creating one (`WorkspaceCreated.adopted`) — that
 *  workspace belongs to someone else's session and its title is not ours
 *  to overwrite. */
async function applyTypedWorkspaceName(
  workspaceId: string,
  typedName: string,
): Promise<void> {
  const name = typedName.trim();
  if (!name) return;
  await renameWorkspace(workspaceId, name).catch((err) => {
    console.warn(
      "[new-workspace-dialog] workspace rename failed (non-fatal):",
      err,
    );
  });
}

/** Why `name` would be rejected by `git check-ref-format --branch`, or null
 *  when it is fine (an empty name is fine: one gets generated). Checked
 *  before the dialog closes so a typo can't cost the user their prompt. */
export function branchNameError(name: string): string | null {
  const branch = name.trim();
  if (!branch) return null;
  if (/\s/.test(branch)) return "Branch names can't contain spaces";
  // eslint-disable-next-line no-control-regex
  if (/[~^:?*[\\\x00-\x1f\x7f]/.test(branch)) {
    return "Branch names can't contain ~ ^ : ? * [ or \\";
  }
  if (branch.includes("..")) return "Branch names can't contain ..";
  if (branch === "@" || branch.includes("@{")) {
    return "Branch names can't be @ or contain @{";
  }
  if (branch === "HEAD") return "HEAD is reserved by git";
  if (branch.startsWith("-")) return "Branch names can't start with -";
  if (branch.startsWith("/") || branch.endsWith("/") || branch.includes("//")) {
    return "Branch names can't start or end with / or contain //";
  }
  if (branch.endsWith(".")) return "Branch names can't end with .";
  for (const part of branch.split("/")) {
    if (part.startsWith(".")) return "No part of a branch name can start with .";
    if (part.endsWith(".lock")) return "No part of a branch name can end with .lock";
  }
  return null;
}

/** Tauri rejects with a bare string; anything else is an Error or unknown. */
function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Every create path in the dialog goes through here. The dialog closes
 *  straight away and a pending sidebar row stands in for the workspace.
 *  On failure that row keeps the form draft, and both it and the error
 *  toast offer "Reopen", which brings the dialog back exactly as it was.
 *  The failed row stays until the reopened draft is submitted again or
 *  the user dismisses it.
 *
 *  `create` resolves to the workspace to activate, or null when it has
 *  already switched somewhere itself. */
async function runCreate(
  closeDialog: () => void,
  displayName: string,
  draft: NewWorkspaceDraft,
  create: () => Promise<string | null>,
): Promise<void> {
  const ui = useUIStore.getState();
  // A retry of a reopened draft replaces its failed row. Read before
  // closing, which clears the reopened draft.
  const reopened = ui.newWorkspaceDraft;
  if (reopened) {
    for (const pw of ui.pendingWorkspaces) {
      if (pw.draft === reopened) ui.removePendingWorkspace(pw.id);
    }
  }
  closeDialog();
  const tempId = randomUUID();
  ui.addPendingWorkspace({
    id: tempId,
    name: displayName,
    projectPath: draft.projectDir,
    status: "creating",
    draft,
  });

  let wsId: string | null;
  try {
    wsId = await create();
  } catch (err) {
    const message = errorText(err);
    ui.failPendingWorkspace(tempId, message);
    toast.error(`Couldn't create workspace: ${message}`, {
      action: {
        label: "Reopen",
        onClick: () => useUIStore.getState().reopenPendingWorkspace(tempId),
      },
    });
    return;
  }

  ui.removePendingWorkspace(tempId);
  if (wsId) {
    await activateWorkspace(wsId).catch((err) => {
      toast.error(`Couldn't open workspace: ${errorText(err)}`);
    });
  }
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function NewWorkspaceDialog({ open, onOpenChange }: Props) {
  const mobile = useMobileLayout();
  const appState = useAppStore((s) => s.appState);
  const activeWorkspaceId = useAppStore(selectActiveWorkspaceId);
  const activeWs = appState?.workspaces.find(
    (w) => w.workspace_id === activeWorkspaceId,
  );
  const storeProjectDir = useUIStore((s) => s.newWorkspaceProjectDir);
  const newWorkspaceDraft = useUIStore((s) => s.newWorkspaceDraft);
  const lastSelectedAgentId = useUIStore((s) => s.lastSelectedAgentId);
  const setLastSelectedAgentId = useUIStore((s) => s.setLastSelectedAgentId);
  const setLastModelSelection = useUIStore((s) => s.setLastModelSelection);

  // Provider capability slots — the live model harvest shared with the
  // Beta agent-chat picker. The launch dialog reads the same data so a
  // model picked here is sourced dynamically (OpenCode live-harvested,
  // Claude/Codex from the maintained bundle).
  const claudeCaps = useProviderCapabilities((s) => s.claude);
  const codexCaps = useProviderCapabilities((s) => s.codex);
  const opencodeCaps = useProviderCapabilities((s) => s.opencode);

  // Gemini isn't a chat provider, so its launch list comes from the
  // backend hybrid harvest (`list_launch_gemini_models`) instead. The
  // init hook kicks a lazy first fetch on dialog mount; subsequent
  // opens reuse the cached value.
  const geminiModels = useLaunchGeminiModels((s) => s.models);
  useLaunchGeminiModelsInit();

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
  // Launch-time model + reasoning override. `{ null, null }` = use the
  // agent's own default (emits no flag). Resolved per agent family by
  // the effect below.
  const [modelSelection, setModelSelection] = useState<ModelSelection>({
    model: null,
    reasoning: null,
    context: null,
  });
  const [baseBranch, setBaseBranch] = useState("main");
  // True once the user has manually picked a branch from the BranchPicker,
  // so the `useDefaultBranch` effect below knows not to clobber their
  // choice when the async detection resolves (or the user re-opens the
  // dialog on the same project). Reset whenever the dialog is reopened or
  // the project changes — see the open/projectDir effect below.
  const userPickedBaseRef = useRef(false);
  const [attachments, setAttachments] = useState<string[]>([]);
  const [linkedIssue, setLinkedIssue] = useState<GitHubIssue | null>(null);
  const [issuePickerOpen, setIssuePickerOpen] = useState(false);
  const [prPickerOpen, setPrPickerOpen] = useState(false);
  const [branchAutoFilled, setBranchAutoFilled] = useState(false);
  const [branchMode, setBranchMode] = useState<"create_new" | "open_existing">("create_new");
  const [openExistingBranch, setOpenExistingBranch] = useState<string | null>(null);
  // Which host the new workspace will run on. `null` = local (this
  // device). Step 2b: the picker writes to this; the actual remote
  // execution wiring happens in step 2d. For now selecting a remote
  // host still creates the workspace locally — the host_id is
  // recorded so the future "Push to host" action can pick it up
  // without re-prompting.
  const [hostId, setHostId] = useState<number | null>(null);

  // Data state
  const [presets, setPresets] = useState<TerminalPreset[]>([]);
  const [localBranches, setLocalBranches] = useState<string[]>([]);
  const [remoteBranches, setRemoteBranches] = useState<string[]>([]);
  const [detailedBranches, setDetailedBranches] = useState<BranchDetail[]>([]);
  const [branchesLoading, setBranchesLoading] = useState(false);
  // True while `git fetch` runs in the background after the local listing.
  const [branchesSyncing, setBranchesSyncing] = useState(false);
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null);
  const [prBranches, setPrBranches] = useState<Set<string>>(new Set());
  const [providerUsable, setProviderUsable] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const issuePickerRef = useRef<HTMLDivElement>(null);
  const prPickerRef = useRef<HTMLDivElement>(null);

  // Tracks project switches mid-dialog; see the re-arm check below.
  const prevProjectDirRef = useRef(projectDir);

  // Reset state when the dialog opens, or restore the draft of a failed
  // create when it was reopened from the pending row or the error toast.
  const prevOpenRef = useRef(false);
  const hydratedDraftRef = useRef<NewWorkspaceDraft | null>(null);
  // A restored draft's model pick, for the model-resolve effect to honour
  // once instead of replacing it with the remembered per-family pick.
  const restoredModelRef = useRef<{
    agentId: string | null;
    selection: ModelSelection;
  } | null>(null);
  // The project this render settles on. `setProjectDir` below only lands on
  // the next render, so the project-switch check must compare against the
  // hydrated dir, not the stale state, or it would treat a Reopen into
  // another project as a mid-dialog switch and drop the draft's base.
  let renderProjectDir = projectDir;
  if (
    open &&
    (!prevOpenRef.current ||
      (newWorkspaceDraft !== null &&
        newWorkspaceDraft !== hydratedDraftRef.current))
  ) {
    const draft = newWorkspaceDraft;
    hydratedDraftRef.current = draft;
    const dir =
      draft?.projectDir ??
      (storeProjectDir || activeWs?.project_root || activeWs?.cwd || "");
    if (projectDir !== dir) setProjectDir(dir);
    renderProjectDir = dir;
    prevProjectDirRef.current = dir;
    setWorkspaceName(draft?.workspaceName ?? "");
    setBranchName(draft?.branchName ?? "");
    setPrompt(draft?.prompt ?? "");
    setSelectedAgentId(
      draft?.selectedAgentId ?? (lastSelectedAgentId || "builtin-claude"),
    );
    if (draft) setModelSelection(draft.modelSelection);
    restoredModelRef.current = draft
      ? { agentId: draft.selectedAgentId, selection: draft.modelSelection }
      : null;
    setBaseBranch(draft?.baseBranch ?? "main");
    // Re-allow auto-adoption of the detected default branch on each open,
    // but keep a restored draft's base exactly as the user left it.
    userPickedBaseRef.current = draft !== null;
    setAttachments(draft?.attachments ?? []);
    setLinkedIssue(draft?.linkedIssue ?? null);
    setIssuePickerOpen(false);
    setPrPickerOpen(false);
    setBranchAutoFilled(draft?.branchAutoFilled ?? false);
    setBranchMode(draft?.branchMode ?? "create_new");
    setOpenExistingBranch(draft?.openExistingBranch ?? null);
    setHostId(draft?.hostId ?? null);
  }
  prevOpenRef.current = open;

  // Resolve the repo's actual default branch (reads `origin/HEAD`, falls
  // back to main/master existence). Returns `null` until the async fetch
  // resolves and on detection failure; we keep "main" as the placeholder
  // for that window so the pill still has something to render.
  const detectedDefaultBranch = useDefaultBranch(projectDir || null);

  // Adopt the detected default whenever it resolves for the current
  // project, unless the user has explicitly picked a different branch
  // from the picker. This fixes the "popup always says main even though
  // the repo's default is master" UX bug, and prevents the create call
  // from failing on repos whose default branch isn't named main.
  useEffect(() => {
    if (!open) return;
    if (userPickedBaseRef.current) return;
    if (!detectedDefaultBranch) return;
    setBaseBranch(detectedDefaultBranch);
  }, [open, detectedDefaultBranch]);

  // Switching projects mid-dialog should re-arm auto-adoption so the new
  // project's default branch wins over a stale pick from the previous
  // project. Tracked separately from the open-reset above because
  // projectDir can change without the dialog closing/reopening.
  if (prevProjectDirRef.current !== renderProjectDir) {
    userPickedBaseRef.current = false;
    prevProjectDirRef.current = renderProjectDir;
  }

  // Load data when dialog opens or project changes
  useEffect(() => {
    if (!open || !projectDir) return;
    let cancelled = false;

    setIsGitRepo(null);
    // Until the new listing lands, attachesToRoot() must not match the
    // previous project's checked-out branch.
    setCurrentBranch(null);
    setLocalBranches([]);
    setRemoteBranches([]);
    setDetailedBranches([]);
    setBranchesLoading(true);
    setBranchesSyncing(false);
    setPrBranches(new Set());
    setProviderUsable(false);

    checkIsGitRepo(projectDir).then((isRepo) => {
      if (cancelled) return;
      setIsGitRepo(isRepo);
      if (!isRepo) { setBranchesLoading(false); return; }

      // Only the newest listing may write state, so a slow first listing
      // can't overwrite the refreshed one that follows the fetch.
      let listingSeq = 0;
      const loadBranches = () => {
        const seq = ++listingSeq;
        return Promise.all([
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
          if (cancelled || seq !== listingSeq) return;
          setLocalBranches(local);
          setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
          setDetailedBranches(detailed);
          setBranchesLoading(false);
          setWorktrees(wt);
          setCurrentBranch(info.branch);
        });
      };

      // Local refs list instantly. The network fetch runs alongside and
      // re-lists only when it succeeds, so offline or a slow VPN never
      // holds the picker on a spinner.
      void loadBranches();
      setBranchesSyncing(true);
      gitFetchPrune(projectDir)
        .then(
          () => (cancelled ? undefined : loadBranches()),
          () => undefined,
        )
        .finally(() => {
          if (!cancelled) setBranchesSyncing(false);
        });

      // Fetch open change requests for branch badges and "+" menu
      // (non-blocking). The gate is the *detected* product's CLI, not
      // `gh` specifically — gating a GitLab project on a gh binary hid
      // affordances glab could serve perfectly well.
      fetchProviderAuth(projectDir)
        .then((status) => {
          if (cancelled) return;
          const usable = status.supported && status.installed;
          setProviderUsable(usable);
          if (!usable) return;
          listPullRequests(projectDir, "open")
            .then((prs) => {
              if (cancelled) return;
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
        if (cancelled) return;
        const cliPresets = snap.presets.filter(
          (p) => p.pinned && p.kind === "cli",
        );
        setPresets(cliPresets);
        setSelectedAgentId((prev) => {
          if (prev && cliPresets.some((p) => p.id === prev)) return prev;
          return (
            cliPresets.find((p) => p.id === "builtin-claude")?.id ??
            cliPresets[0]?.id ??
            "builtin-claude"
          );
        });
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

  // When the branch we're about to create already belongs to a workspace,
  // surface it up-front instead of silently deduping at submit. Linking an
  // issue auto-fills a deterministic branch name, so this is how the user
  // learns "you already have a workspace for issue #N" before hitting send.
  // Scoped to create_new — the open_existing flow is an explicit "open it"
  // choice already.
  const existingWorkspaceForBranch = useMemo(() => {
    if (branchMode !== "create_new") return undefined;
    const branch = branchName.trim();
    return branch ? branchWorkspaceMap.get(branch) : undefined;
  }, [branchMode, branchName, branchWorkspaceMap]);

  const handleOpenExistingWorkspace = useCallback(
    async (wsId: string) => {
      onOpenChange(false);
      await activateWorkspaceInteraction(wsId);
    },
    [onOpenChange],
  );

  // Worktree paths already owned by a Codemux workspace. Used to filter
  // `git worktree list` so "Open ↵ <branch>" doesn't try to re-import a
  // worktree that's already attached to another workspace row.
  const existingWorktreePaths = useMemo(() => {
    const set = new Set<string>();
    for (const ws of appState?.workspaces ?? []) {
      if (ws.worktree_path) set.add(ws.worktree_path);
    }
    return set;
  }, [appState]);

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

  // Hosting product of the project being branched from, taken from any
  // already-open workspace on it. Null before the project has one, in
  // which case every consumer falls back to GitHub wording.
  const projectProviderKind = useMemo(() => {
    if (!appState || !projectDir) return null;
    const ws = appState.workspaces.find(
      (w) => w.project_root === projectDir || w.cwd === projectDir,
    );
    return ws?.provider_kind ?? null;
  }, [appState, projectDir]);

  // Whether the issue picker is available: a supported product backs
  // this checkout and its CLI is installed.
  const repoSupported = providerUsable;

  // Selected agent preset
  const selectedAgent = useMemo(
    () => presets.find((p) => p.id === selectedAgentId) ?? null,
    [presets, selectedAgentId],
  );

  // ── Launch-time model selection ──────────────────────────────────
  // Detect the agent family from the selected preset's command. A
  // preset that launches an already-modeled CLI lights up the model
  // pill automatically; an unknown binary leaves `launchFamily` null
  // and the pill stays hidden.
  const launchFamily = useMemo(
    () => detectLaunchFamily(selectedAgent?.commands?.[0]),
    [selectedAgent],
  );
  const launchProviderKind = launchFamily
    ? familyToProviderKind(launchFamily)
    : null;
  const launchCapabilitiesLoaded = useProviderCapabilities((state) =>
    launchProviderKind
      ? selectProviderCapabilitiesLoaded(state, launchProviderKind)
      : true,
  );
  // The capability bundle for the selected family (Gemini has none).
  const launchCaps =
    launchFamily === "claude"
      ? claudeCaps
      : launchFamily === "codex"
        ? codexCaps
        : launchFamily === "opencode"
          ? opencodeCaps
          : null;

  // Model list: Gemini routes through the backend hybrid harvest
  // (`list_launch_gemini_models`) — live from Google's API when
  // GEMINI_API_KEY is set, otherwise the maintained fallback. The
  // frontend `GEMINI_MODELS` const stays as a paper backstop for the
  // window between mount and the first fetch resolving. Every other
  // family reads the shared chat-capability harvest.
  const launchModels = useMemo<LaunchModel[]>(() => {
    if (launchFamily === "gemini") return geminiModels ?? GEMINI_MODELS;
    return (
      launchCaps?.models.map((m) => ({
        id: m.id,
        label: m.label,
        subProvider: m.sub_provider,
      })) ?? []
    );
  }, [launchFamily, launchCaps, geminiModels]);

  const launchModelsLoading =
    launchFamily !== null &&
    launchFamily !== "gemini" &&
    launchProviderKind !== null &&
    !launchCapabilitiesLoaded &&
    launchModels.length === 0;

  // Reasoning + context options are read live from the *selected*
  // model's capability entry, so the reasoning/context pill reflects
  // exactly what that model supports. There is deliberately no
  // first-model fallback: on "Default" (no concrete model) this is null,
  // so the reasoning/context pill hides — reasoning/context belong to a
  // chosen model and shouldn't be pickable before one is selected.
  const launchCapsModel = useMemo(() => {
    if (!launchCaps) return null;
    return (
      launchCaps.models.find((m) => m.id === modelSelection.model) ?? null
    );
  }, [launchCaps, modelSelection.model]);

  // Reasoning levels — dynamic from the model's `effort_levels`, gated
  // to the families whose CLI actually exposes a reasoning flag.
  const reasoningOptions = useMemo<ReasoningOption[]>(() => {
    if (!launchFamily || !REASONING_FLAG_FAMILIES.has(launchFamily)) return [];
    const labels = launchCaps?.effort_label_map ?? {};
    return (launchCapsModel?.effort_levels ?? []).map((lvl) => ({
      value: lvl,
      label: labels[lvl] ?? lvl,
    }));
  }, [launchFamily, launchCaps, launchCapsModel]);

  // Context-window options — dynamic from the model's
  // `context_window_options`. The capability bundle only populates
  // these for Claude, so other families get an empty list (no row).
  const launchContextOptions = useMemo<ReasoningOption[]>(
    () =>
      (launchCapsModel?.context_window_options ?? []).map((o) => ({
        value: o.value,
        label: o.label,
      })),
    [launchCapsModel],
  );

  // Drop a stored reasoning / context value the current model no longer
  // supports (e.g. after switching from Opus to Sonnet). While the
  // capability harvest is still in flight there is no model entry to
  // validate against — pass the stored value through untouched rather
  // than treating "unknown" as "unsupported", which would silently
  // drop (and then re-persist away) a remembered pick.
  const capsReady = launchFamily === "gemini" || launchCaps !== null;
  const effectiveReasoning =
    !capsReady ||
    reasoningOptions.some((o) => o.value === modelSelection.reasoning)
      ? modelSelection.reasoning
      : null;
  const effectiveContext =
    !capsReady ||
    launchContextOptions.some((o) => o.value === modelSelection.context)
      ? modelSelection.context
      : null;

  // Opening the launch surface is explicit provider intent. Paint a persisted
  // catalog immediately, but refresh it once in the background even when the
  // cached slot is non-null so model additions/removals are not frozen for the
  // renderer's lifetime.
  useEffect(() => {
    if (!open || !launchProviderKind) return;
    void refreshProviderCapabilitiesForIntent(launchProviderKind);
  }, [open, launchProviderKind]);

  // Resolve the model selection when the dialog opens, when the chosen
  // agent changes, or once the preset list first loads. Deliberately
  // keyed on `selectedAgentId` + `presetsLoaded` (primitives) rather
  // than the `selectedAgent` object: a mid-dialog `presets` refresh
  // (e.g. switching project) must NOT clobber the user's in-dialog pick.
  const presetsLoaded = presets.length > 0;
  useEffect(() => {
    if (!open || !presetsLoaded) return;
    const restored = restoredModelRef.current;
    restoredModelRef.current = null;
    if (restored && restored.agentId === selectedAgentId) {
      setModelSelection(restored.selection);
      return;
    }
    const cmd = presets.find((p) => p.id === selectedAgentId)?.commands?.[0];
    const family = detectLaunchFamily(cmd);
    if (!family) {
      setModelSelection({ model: null, reasoning: null, context: null });
      return;
    }
    const remembered = useUIStore.getState().lastModelSelections[family];
    if (remembered) {
      // Normalise: entries persisted before `context` existed lack it.
      setModelSelection({
        model: remembered.model ?? null,
        reasoning: remembered.reasoning ?? null,
        context: remembered.context ?? null,
      });
    } else {
      setModelSelection({
        model: parseBakedModel(cmd),
        reasoning: null,
        context: null,
      });
    }
    // `presets` is read but intentionally not a dependency — see above.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, selectedAgentId, presetsLoaded]);

  // Auto-resize textarea (min 96px = ~5 lines, max 192px = ~10 lines)
  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setPrompt(e.target.value);
    const ta = e.target;
    ta.style.height = "auto";
    ta.style.height = `${Math.min(Math.max(ta.scrollHeight, 96), 192)}px`;
  };

  // Accept clipboard images alongside the paperclip-attach flow.
  //
  // Linux/WebKit2GTK strips image payloads from the standard `paste`
  // event for security reasons, so we cannot read clipboard images
  // from JS at all. We delegate the entire flow to Rust: a single
  // `paste_clipboard_image_to_file` command reads the OS clipboard,
  // encodes a real PNG, writes it to the codemux temp dir, and
  // returns just the file path. The image bytes never cross the IPC
  // boundary, which keeps Ctrl+V snappy even for large screenshots.
  //
  // The downstream attachment pipeline already takes filesystem
  // paths (the paperclip flow opens a file picker), so the returned
  // path drops straight into the existing `attachments` array. Chip
  // rendering, X-to-remove, and prompt inlining at submit stay
  // identical to a file-picked image.
  const handlePasteImage = useCallback(
    async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      let path: string;
      try {
        path = await pasteClipboardImageToFile();
      } catch {
        // No image on clipboard (or plugin error). Let the textarea
        // handle the paste the normal way — typing plain text, etc.
        return;
      }

      if (!path) return;

      // We DO have an image. Prevent the default paste so the bytes
      // don't also get rendered as text in the textarea.
      e.preventDefault();

      setAttachments((prev) => {
        if (prev.includes(path)) return prev;
        return [...prev, path];
      });
    },
    [],
  );

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

  const handlePrSelect = useCallback((pr: PullRequestInfo) => {
    // Linking a PR fills the branch from its head ref so the workspace
    // tracks the PR's branch. Mark it as an explicit (non-auto) choice
    // so a later issue link won't clobber it.
    if (pr.head_branch) {
      setBranchName(pr.head_branch);
      setBranchAutoFilled(false);
    }
  }, []);

  const handleOpenExisting = useCallback((branch: string) => {
    setBranchMode("open_existing");
    setOpenExistingBranch(branch);
    setBranchName("");
  }, []);

  // A branch that should attach to the repo root rather than get its own
  // worktree: the repo's default (as the picker labels it) or whatever the
  // root has checked out, since git refuses a second checkout of either.
  const attachesToRoot = (branch: string) =>
    branch === currentBranch ||
    (detectedDefaultBranch
      ? branch === detectedDefaultBranch
      : branch === "main" || branch === "master");

  // Non-git folders run in the folder itself, so there is no branch to name.
  const branchError =
    branchMode === "create_new" && isGitRepo !== false
      ? branchNameError(branchName)
      : null;
  const canSubmit = !!projectDir && !branchError;

  const snapshotDraft = (): NewWorkspaceDraft => ({
    projectDir,
    workspaceName,
    branchName,
    branchAutoFilled,
    prompt,
    attachments,
    linkedIssue,
    selectedAgentId,
    modelSelection,
    baseBranch,
    branchMode,
    openExistingBranch,
    hostId,
  });
  const closeDialog = () => onOpenChange(false);

  const handleSubmit = async () => {
    if (!projectDir || branchError) return;

    // Snapshot before any await so Reopen restores exactly what was typed.
    const draft = snapshotDraft();

    // Build user prompt with attachments
    let userPrompt = attachments.length > 0
      ? `${prompt.trim()}\n\nAttached files:\n${attachments.map((f) => `- ${f}`).join("\n")}`
      : prompt.trim();

    // Inject linked issue context into prompt.
    // Always use path-based lookup to avoid stale workspace resolution.
    if (linkedIssue) {
      let issueBody: string | null = null;
      try {
        const full = await getGithubIssueByPath(projectDir, linkedIssue.number);
        issueBody = full.body ?? null;
      } catch {
        // Non-blocking: proceed without body
      }
      userPrompt = buildPromptWithIssueContext(
        userPrompt,
        linkedIssue,
        issueBody,
        resolveProvider(projectProviderKind).name,
      );
    }

    const fullPrompt = userPrompt;

    // Launch-time model selection — only meaningful when the chosen
    // agent is a family Codemux can inject model flags for. The 1M
    // context window rides on the model id (`model[1m]`), so it needs a
    // concrete model: when the user left the model on "Default",
    // resolve to the capability default, and drop 1M if that model
    // can't do it (e.g. Haiku). Remember the pick per family so
    // reopening the dialog restores it.
    let resolvedModel = modelSelection.model;
    let resolvedContext = effectiveContext;
    if (launchFamily === "claude" && resolvedContext === "1m") {
      const target = resolvedModel ?? claudeCaps?.models[0]?.id ?? null;
      const supports1m = !!claudeCaps?.models
        .find((m) => m.id === target)
        ?.context_window_options.some((o) => o.value === "1m");
      if (target && supports1m) {
        resolvedModel = target;
      } else {
        resolvedContext = null;
      }
    }
    const resolvedSelection: ModelSelection = {
      model: resolvedModel,
      reasoning: effectiveReasoning,
      context: resolvedContext,
    };
    const launchSelection: ModelSelection | null = launchFamily
      ? resolvedSelection
      : null;
    if (launchFamily) {
      // Persist the user's *literal* pick, not the launch-resolved one
      // (which may have dropped 1M or substituted a default model when
      // capabilities had not loaded). This keeps the saved preference
      // intact across reopens regardless of harvest timing.
      setLastModelSelection(launchFamily, modelSelection);
    }

    // The git probe may still be in flight on a quick submit; a non-git
    // folder must not fall through to the worktree path.
    const gitRepo =
      isGitRepo ?? (await checkIsGitRepo(projectDir).catch(() => true));

    const displayName =
      workspaceName || prompt.slice(0, 40) || openExistingBranch || branchName || "New workspace";

    // Where the workspace lands. Null means an existing workspace already
    // owns the branch and has been focused instead.
    type Placed = { wsId: string; agentHandled: boolean; adopted: boolean };
    const placeWorkspace = async (): Promise<Placed | null> => {
      // A plain folder: the workspace runs in it directly, no branch.
      if (!gitRepo) {
        const wsId = await createWorkspace(projectDir);
        return { wsId, agentHandled: false, adopted: false };
      }

      // Open existing branch mode — skip branch generation
      if (branchMode === "open_existing" && openExistingBranch) {
        const existingWsId = branchWorkspaceMap.get(openExistingBranch);
        if (existingWsId) {
          toast.info(
            linkedIssue
              ? `Issue #${linkedIssue.number} already has a workspace — switched to it.`
              : `"${openExistingBranch}" already has a workspace — switched to it.`,
          );
          await activateWorkspaceInteraction(existingWsId);
          return null;
        }

        // A real orphan is a worktree on disk whose branch we want, that isn't the
        // main repo itself and isn't already owned by an existing Codemux workspace.
        // Both filters are required: the first excludes the primary repo (which appears
        // in `git worktree list --porcelain`), the second excludes worktrees we already
        // manage as a workspace.
        const orphan = worktrees.find(
          (wt) =>
            (wt.branch === openExistingBranch ||
              wt.branch === `refs/heads/${openExistingBranch}`) &&
            wt.path !== projectDir &&
            !existingWorktreePaths.has(wt.path),
        );

        if (orphan) {
          const wsId = await importWorktreeWorkspace(orphan.path, openExistingBranch, "single");
          return { wsId, agentHandled: false, adopted: false };
        }
        if (attachesToRoot(openExistingBranch)) {
          // Open on the default branch always attaches to the real repo root.
          // The sidebar label will reflect actual HEAD via the live refresh loop,
          // so the user sees reality. No phantom worktree is created.
          const wsId = await createWorkspace(projectDir);
          return { wsId, agentHandled: false, adopted: false };
        }
        const created = await createWorktreeWorkspaceResult(
          projectDir,
          openExistingBranch,
          false,
          "single",
          null,
          fullPrompt || null,
          selectedAgentId,
          null,
          launchSelection,
        );
        // The backend adopted an existing live workspace for this worktree
        // instead of creating one — the prompt/preset were deliberately
        // dropped (injecting into an in-flight session is worse). Silence
        // here would read as "nothing happened" and quietly lose the
        // typed message, so say so.
        if (created.adopted) {
          toast.info(
            fullPrompt
              ? `"${openExistingBranch}" already has a live workspace — switched to it. Your prompt wasn't sent.`
              : `"${openExistingBranch}" already has a live workspace — switched to it.`,
          );
        }
        return { wsId: created.workspaceId, agentHandled: true, adopted: created.adopted };
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

      // Check if workspace already exists for this branch. Linking an issue
      // auto-fills a deterministic `feature/<n>-<slug>` branch name, so
      // re-linking an issue that already has a workspace lands here. Switch
      // to it AND tell the user: a silent return reads as "nothing happened"
      // and quietly drops the message they just typed.
      const existingWsId = branchWorkspaceMap.get(resolvedBranch);
      if (existingWsId) {
        toast.info(
          linkedIssue
            ? `Issue #${linkedIssue.number} already has a workspace — switched to it.`
            : `"${resolvedBranch}" already has a workspace — switched to it.`,
        );
        await activateWorkspaceInteraction(existingWsId);
        return null;
      }

      // Existing default (or checked-out) branch: attach to the real repo
      // root. The sidebar branch label reflects actual HEAD via the live
      // refresh loop, so the user sees reality instead of a phantom
      // worktree at ~/.codemux/worktrees/<project>/<branch>. Feature
      // branches always get a proper worktree.
      if (!isNewBranch && attachesToRoot(resolvedBranch)) {
        const wsId = await createWorkspace(projectDir);
        return { wsId, agentHandled: false, adopted: false };
      }

      // Same orphan filter as the open-existing flow above: skip the main
      // repo entry (which `git worktree list` includes) and any worktree
      // already owned by another workspace.
      const orphan = worktrees.find(
        (wt) =>
          (wt.branch === resolvedBranch ||
            wt.branch === `refs/heads/${resolvedBranch}`) &&
          wt.path !== projectDir &&
          !existingWorktreePaths.has(wt.path),
      );
      if (orphan) {
        const wsId = await importWorktreeWorkspace(
          orphan.path,
          resolvedBranch,
          "single",
        );
        return { wsId, agentHandled: false, adopted: false };
      }

      const created = await createWorktreeWorkspaceResult(
        projectDir,
        resolvedBranch,
        isNewBranch,
        "single",
        isNewBranch ? baseBranch || null : null,
        fullPrompt || null,
        selectedAgentId,
        null,
        launchSelection,
      );
      // Adopt path: an existing live workspace claims this worktree, so
      // the backend focused it and dropped the prompt/preset instead of
      // typing into its in-flight session. Surface that — otherwise the
      // user's prompt disappears silently.
      if (created.adopted) {
        toast.info(
          fullPrompt
            ? `"${resolvedBranch}" already has a live workspace — switched to it. Your prompt wasn't sent.`
            : `"${resolvedBranch}" already has a live workspace — switched to it.`,
        );
      }
      return { wsId: created.workspaceId, agentHandled: true, adopted: created.adopted };
    };

    await runCreate(closeDialog, displayName, draft, async () => {
      const placed = await placeWorkspace();
      if (!placed) return null;
      const { wsId, agentHandled, adopted } = placed;

      if (!adopted) {
        await applyTypedWorkspaceName(wsId, workspaceName);
      }

      // Launch agent for paths that don't handle it internally
      if (!agentHandled && selectedAgentId && fullPrompt) {
        await applyPreset(
          wsId,
          selectedAgentId,
          "current_terminal",
          fullPrompt,
          launchSelection,
        );
      }

      // Track as recent project
      dbAddRecentProject(projectDir, basename(projectDir)).catch(console.error);

      // Link issue to the new workspace
      if (linkedIssue) {
        try {
          await linkWorkspaceIssue(wsId, linkedIssue.number);
        } catch (linkErr) {
          console.error("Failed to link issue:", linkErr);
          toast.warning("Workspace created but issue linking failed. You can re-link from the workspace.");
        }
      }

      // Persist host_id on the new workspace. Best-effort: a failed
      // call only loses the device assignment, not the workspace
      // itself — and the user can re-pick the host from the
      // workspace header badge.
      if (hostId !== null) {
        try {
          await setWorkspaceHost(wsId, hostId);
        } catch (hostErr) {
          console.error("Failed to set workspace host:", hostErr);
        }
      }

      return wsId;
    });
  };

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

  // Close PR picker on click outside it (within the dialog)
  useEffect(() => {
    if (!prPickerOpen) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (prPickerRef.current && !prPickerRef.current.contains(e.target as Node)) {
        setPrPickerOpen(false);
      }
    };
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [prPickerOpen]);

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
        data-mobile-workspace-dialog
        showCloseButton={false}
        className={cn("sm:max-w-2xl bg-popover p-0 gap-0 overflow-visible", !mobile && "max-h-[min(70vh,600px)] !top-[calc(50%-min(35vh,300px))] !-translate-y-0")}
        onKeyDown={handleKeyDown}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>New Workspace</DialogTitle>
          <DialogDescription>
            Create a new workspace from a prompt
          </DialogDescription>
        </DialogHeader>

        <div className="mobile-workspace-dialog-heading"><strong>New workspace</strong><button type="button" onClick={() => onOpenChange(false)}>Cancel</button></div>
        {/* Top row: workspace name + branch name as quiet inline fields.
            A non-git folder has no branch, so that field is omitted. */}
        <div className="flex items-center gap-3 px-4 pt-3 pb-0.5">
          <Input
            value={workspaceName}
            onChange={(e) => setWorkspaceName(e.target.value)}
            placeholder="Workspace name (optional)"
            className="h-6 text-label flex-1 border-0 bg-transparent dark:bg-transparent px-0 shadow-none focus-visible:ring-0 text-muted-foreground placeholder:text-muted-foreground/60"
          />
          {isGitRepo === false ? null : branchMode === "create_new" ? (
            <label
              className="flex h-6 min-w-0 shrink-0 items-center gap-1 text-muted-foreground"
              title={branchName || undefined}
            >
              <GitBranch className="size-3 shrink-0 opacity-60" />
              <Input
                value={branchName}
                onChange={(e) => { setBranchName(e.target.value); setBranchAutoFilled(false); }}
                placeholder="branch name"
                aria-label="Branch name"
                aria-invalid={branchError ? true : undefined}
                aria-describedby={branchError ? "new-workspace-branch-error" : undefined}
                // Monospace, so `ch` sizes the field to its content and an
                // issue-derived name shows in full up to the cap.
                style={{ width: `${Math.max(branchName.length, 11) + 1}ch` }}
                className="h-6 max-w-[260px] text-label border-0 bg-transparent dark:bg-transparent px-0 shadow-none focus-visible:ring-0 aria-invalid:ring-0 aria-invalid:text-destructive font-mono text-muted-foreground placeholder:text-muted-foreground/60"
              />
            </label>
          ) : (
            <span
              className="h-6 text-label text-right font-mono text-muted-foreground/60 flex items-center truncate max-w-[260px]"
              title={openExistingBranch ?? undefined}
            >
              on {openExistingBranch}
            </span>
          )}
        </div>
        {branchError && (
          <p
            id="new-workspace-branch-error"
            className="px-4 text-right text-caption text-destructive"
          >
            {branchError}
          </p>
        )}

        {/* Center: prompt textarea with embedded controls */}
        <div className="relative px-3 pt-2 pb-3">
          <div className="rounded-lg border border-border bg-muted overflow-hidden">
            <Textarea
              ref={textareaRef}
              value={prompt}
              onChange={handleTextareaChange}
              onPaste={handlePasteImage}
              placeholder="What do you want to do?"
              className="min-h-24 max-h-48 resize-none border-0 bg-transparent dark:bg-transparent shadow-none focus-visible:ring-0 text-body px-4 pt-3 pb-1"
              rows={1}
            />

            {/* Attachment chips + linked issue chip */}
            {(attachments.length > 0 || linkedIssue) && (
              <div className="flex flex-wrap gap-2 px-4 pb-2.5">
                {/* Linked issue chip */}
                {linkedIssue && (
                  <span
                    className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 py-0.5 pl-1 pr-1 text-label text-foreground"
                    title={`#${linkedIssue.number} ${linkedIssue.title}`}
                  >
                    <span
                      className={cn(
                        "flex size-5 shrink-0 items-center justify-center rounded-sm",
                        linkedIssue.state === "Open"
                          ? "bg-success/15 text-success"
                          : "bg-surface-3 text-muted-foreground",
                      )}
                    >
                      <CircleDot className="size-3" />
                    </span>
                    <span className="font-mono tabular-nums text-muted-foreground">
                      #{linkedIssue.number}
                    </span>
                    <span className="max-w-[160px] truncate">{linkedIssue.title}</span>
                    <button
                      type="button"
                      aria-label={`Remove issue #${linkedIssue.number}`}
                      className="ml-0.5 rounded-full p-0.5 text-muted-foreground/70 transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
                      onClick={() => {
                        setLinkedIssue(null);
                        if (branchAutoFilled) {
                          setBranchName("");
                          setBranchAutoFilled(false);
                        }
                      }}
                    >
                      <X className="size-3" />
                    </button>
                  </span>
                )}
                {attachments.map((file) => (
                  <WorkspaceAttachmentChip
                    key={file}
                    path={file}
                    onRemove={() =>
                      setAttachments((prev) => prev.filter((f) => f !== file))
                    }
                  />
                ))}
              </div>
            )}

            {/* Already-exists notice: the linked issue (or typed branch)
                already has a workspace. Offer to open it instead of the
                silent submit-time dedup that drops the typed message. */}
            {existingWorkspaceForBranch && (
              <div className="mx-3 mb-2 flex items-center justify-between gap-2 rounded-md border border-border bg-muted/40 px-3 py-1.5 text-label text-muted-foreground">
                <span className="min-w-0 truncate">
                  {linkedIssue
                    ? `Issue #${linkedIssue.number} already has a workspace.`
                    : `"${branchName.trim()}" already has a workspace.`}
                </span>
                <button
                  type="button"
                  onClick={() =>
                    handleOpenExistingWorkspace(existingWorkspaceForBranch)
                  }
                  className="shrink-0 rounded-md border border-border bg-background px-2 py-0.5 font-medium text-foreground transition-colors duration-150 hover:bg-muted"
                >
                  Open it
                </button>
              </div>
            )}

            {/* Footer inside textarea border */}
            <div className="flex items-center justify-between px-3 pb-3 pt-0" data-mobile-workspace-controls>
              <div className="flex items-center gap-2 min-w-0">
                {/* Agent picker — pill with real icon. The DEVICE
                    picker used to live here too, but it belongs
                    with project + branch in the row below — those
                    are all "workspace identity" choices, while the
                    agent is "session content." See bottom row. */}
                <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-label text-foreground transition-colors duration-150 hover:bg-muted"
                  >
                    {selectedAgent ? (
                      <>
                        <PresetIcon icon={selectedAgent.icon} className="size-3.5" />
                        {selectedAgent.name}
                      </>
                    ) : (
                      <>
                        <PresetIcon icon="claude" className="size-3.5" />
                        Claude Code
                      </>
                    )}
                    <ChevronDown className="size-3 opacity-40" />
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

              {/* Model picker — appears only for agents whose CLI
                  Codemux can inject a `--model` flag for. Sourced from
                  the same capability harvest as the Beta chat picker. */}
              {launchFamily && (
                <>
                  <LaunchModelPicker
                    providerKind={launchProviderKind}
                    models={launchModels}
                    loading={launchModelsLoading}
                    selectedModel={modelSelection.model}
                    onModelChange={(model) =>
                      // Picking "Default" (null) clears reasoning/context
                      // too — they're attributes of a concrete model.
                      setModelSelection((prev) =>
                        model === null
                          ? { model: null, reasoning: null, context: null }
                          : { ...prev, model },
                      )
                    }
                  />
                  {/* Reasoning/context for the chosen model — a sibling
                      pill that hides on Default and for models with no
                      options (Haiku), mirroring the chat composer. */}
                  <LaunchReasoningPicker
                    reasoningOptions={reasoningOptions}
                    selectedReasoning={effectiveReasoning}
                    defaultReasoning={launchCapsModel?.default_effort ?? null}
                    onReasoningChange={(reasoning) =>
                      setModelSelection((prev) => ({ ...prev, reasoning }))
                    }
                    contextOptions={launchContextOptions}
                    selectedContext={effectiveContext}
                    defaultContext={
                      launchCapsModel?.context_window_options.find(
                        (o) => o.is_default,
                      )?.value ?? null
                    }
                    onContextChange={(context) =>
                      setModelSelection((prev) => ({ ...prev, context }))
                    }
                  />
                </>
              )}
              </div>

              <div className="flex items-center gap-1">
                {/* Attach files */}
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      aria-label="Attach files"
                      className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
                      onClick={async () => {
                        const files = await pickFiles("Attach files");
                        if (files.length > 0) {
                          setAttachments((prev) => {
                            const existing = new Set(prev);
                            return [...prev, ...files.filter((f) => !existing.has(f))];
                          });
                        }
                      }}
                    >
                      <Paperclip className="size-4" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="top">Attach files</TooltipContent>
                </Tooltip>

                {/* Link pull request */}
                {providerUsable && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        aria-label="Link pull request"
                        className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
                        onClick={() => {
                          setIssuePickerOpen(false);
                          setPrPickerOpen(true);
                        }}
                      >
                        <GitPullRequest className="size-4" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="top">Link pull request</TooltipContent>
                  </Tooltip>
                )}

                {/* Link issue */}
                {repoSupported && !linkedIssue && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        aria-label="Link issue"
                        className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
                        onClick={() => {
                          setPrPickerOpen(false);
                          setIssuePickerOpen(true);
                        }}
                      >
                        <CircleDot className="size-4" />
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
                      className={cn(
                        "inline-flex size-8 items-center justify-center rounded-full border transition-colors duration-100 disabled:pointer-events-none disabled:opacity-50",
                        canSubmit
                          ? "border-transparent bg-foreground text-background hover:bg-foreground/90"
                          : "border-border bg-muted text-muted-foreground",
                      )}
                      onClick={handleSubmit}
                      disabled={!canSubmit}
                    >
                      <ArrowUp className="size-4" />
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
                providerKind={projectProviderKind}
                open={issuePickerOpen}
                onSelect={handleIssueSelect}
                onClose={() => setIssuePickerOpen(false)}
              />
            </div>
          )}

          {/* PR picker — same floating treatment as the issue picker */}
          {prPickerOpen && projectDir && (
            <div
              ref={prPickerRef}
              className="absolute right-0 top-full mt-1 z-50 w-[320px] rounded-lg border border-border bg-popover shadow-lg overflow-hidden animate-in fade-in-0 zoom-in-95 duration-150"
            >
              <PrPickerPanel
                projectPath={projectDir}
                providerKind={projectProviderKind}
                open={prPickerOpen}
                onSelect={handlePrSelect}
                onClose={() => setPrPickerOpen(false)}
              />
            </div>
          )}
        </div>

        {/* Bottom row: device + project + branch pickers as muted
            pills. All three are "workspace identity" choices — on
            what device, what project, on what branch. Device
            comes leftmost because picking "where" constrains
            everything downstream (project list, branch list). The
            agent picker is a separate tier (session content) and
            stays inside the textarea footer above. */}
        <div className="flex items-center gap-2 px-4 pb-3">
          {/* Device picker — leftmost in the identity row. `null`
              = local. Styled to match the project + branch pills
              (rounded-full, bg-muted/60, ChevronDown). */}
          <DevicePicker hostId={hostId} onSelectHostId={setHostId} />

          <ProjectPicker
            value={projectDir || null}
            onChange={(path) => setProjectDir(path)}
          />

          {/* Base branch picker. A plain folder has no branches, so say
              why the pill is missing and where the workspace will run. */}
          {isGitRepo === false ? (
            <span
              className="inline-flex min-w-0 items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-label text-muted-foreground"
              title="This folder isn't a git repository, so no branch or worktree is created. The agent works in the folder directly."
            >
              <FolderOpen className="size-3 shrink-0" />
              <span className="truncate">Not a git repo · runs in the folder</span>
            </span>
          ) : (
            <BranchPicker
              baseBranch={openExistingBranch || baseBranch}
              branches={detailedBranches}
              worktrees={worktrees}
              branchWorkspaceMap={branchWorkspaceMap}
              prBranches={prBranches}
              currentBranch={currentBranch}
              defaultBranchName={detectedDefaultBranch}
              loading={branchesLoading}
              syncing={branchesSyncing}
              onSelectBase={(branch) => {
                userPickedBaseRef.current = true;
                setBaseBranch(branch);
                setBranchMode("create_new");
                setOpenExistingBranch(null);
              }}
              onOpenWorkspace={(wsId) => {
                closeDialog();
                activateWorkspaceInteraction(wsId).catch((err) => {
                  toast.error(`Couldn't open workspace: ${errorText(err)}`);
                });
              }}
              onImportWorktree={(path, branch) => {
                void runCreate(closeDialog, branch, snapshotDraft(), () =>
                  importWorktreeWorkspace(path, branch, "single"),
                );
              }}
              onCreateOnCurrent={() => {
                void runCreate(
                  closeDialog,
                  workspaceName || currentBranch || basename(projectDir),
                  snapshotDraft(),
                  async () => {
                    const wsId = await createWorkspace(projectDir);
                    // Same contract as the main create paths: honour a
                    // typed name before the workspace becomes visible.
                    // Nothing else ever names this one — it cuts no
                    // branch — so without this it keeps the backend
                    // default (the directory's name) forever.
                    await applyTypedWorkspaceName(wsId, workspaceName);
                    return wsId;
                  },
                );
              }}
              onOpenExisting={handleOpenExisting}
              isOpenMode={branchMode === "open_existing"}
            />
          )}

          <span className="ml-auto flex shrink-0 items-center gap-1 text-caption text-muted-foreground select-none">
            <kbd className="rounded-sm bg-surface-2 px-1 font-mono text-micro text-foreground/80">
              Ctrl+Enter
            </kbd>{" "}
            to create
          </span>
        </div>

      </DialogContent>
    </Dialog>
  );
}
