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
} from "lucide-react";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { PresetIcon } from "@/components/icons/preset-icon";
import { ProjectPicker } from "./project-picker";
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
  createWorktreeWorkspace,
  importWorktreeWorkspace,
  setWorkspaceHost,
  activateWorkspace,
  getPresets,
  checkIsGitRepo,
  dbAddRecentProject,
  generateBranchName,
  generateRandomBranchName,
  checkGhAvailable,
  checkGithubRepo,
  listPullRequests,
  pasteClipboardImageToFile,
  suggestIssueBranchName,
  linkWorkspaceIssue,
  getGithubIssueByPath,
  applyPreset,
} from "@/tauri/commands";
import { pickFiles } from "@/lib/file-dialog";
import type { TerminalPreset, WorktreeInfo, BranchDetail, PullRequestInfo, GitHubIssue, LinkedIssue, ModelSelection } from "@/tauri/types";
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
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import {
  useLaunchGeminiModels,
  useLaunchGeminiModelsInit,
} from "@/stores/gemini-models-store";

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
  const setLastModelSelection = useUIStore((s) => s.setLastModelSelection);

  // Provider capability slots — the live model harvest shared with the
  // Beta agent-chat picker. The launch dialog reads the same data so a
  // model picked here is sourced dynamically (OpenCode live-harvested,
  // Claude/Codex from the maintained bundle).
  const claudeCaps = useProviderCapabilities((s) => s.claude);
  const codexCaps = useProviderCapabilities((s) => s.codex);
  const opencodeCaps = useProviderCapabilities((s) => s.opencode);
  const capsLoaded = useProviderCapabilities((s) => s.loaded);
  const refreshCaps = useProviderCapabilities((s) => s.refresh);

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
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([]);
  const [currentBranch, setCurrentBranch] = useState<string | null>(null);
  const [isGitRepo, setIsGitRepo] = useState<boolean | null>(null);
  const [prBranches, setPrBranches] = useState<Set<string>>(new Set());
  const [ghAvailable, setGhAvailable] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const issuePickerRef = useRef<HTMLDivElement>(null);
  const prPickerRef = useRef<HTMLDivElement>(null);

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
    // Re-allow auto-adoption of the detected default branch on each open.
    userPickedBaseRef.current = false;
    setAttachments([]);
    setLinkedIssue(null);
    setIssuePickerOpen(false);
    setPrPickerOpen(false);
    setBranchAutoFilled(false);
    setBranchMode("create_new");
    setOpenExistingBranch(null);
    setHostId(null);
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
  const prevProjectDirRef = useRef(projectDir);
  if (prevProjectDirRef.current !== projectDir) {
    userPickedBaseRef.current = false;
    prevProjectDirRef.current = projectDir;
  }

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

      // Fetch remote refs first so branch list and commits are current.
      // Fail gracefully — stale local refs are still usable.
      const fetchDone = gitFetchPrune(projectDir).catch(() => {});

      fetchDone.then(() => {
        if (cancelled) return;
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
          if (cancelled) return;
          setLocalBranches(local);
          setRemoteBranches(remote.map((b) => b.replace(/^origin\//, "")));
          setDetailedBranches(detailed);
          setBranchesLoading(false);
          setWorktrees(wt);
          setCurrentBranch(info.branch);
        });
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
      await activateWorkspace(wsId);
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

  // Whether the issue picker is available (needs gh + a GitHub repo)
  const isGithubRepo = ghAvailable;

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
    !capsLoaded &&
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

  // Backstop the app-level capability harvest: if the dialog opens
  // before a provider's slot has hydrated, kick a refresh for it.
  useEffect(() => {
    if (!open || !launchProviderKind) return;
    const caps =
      launchProviderKind === "claude"
        ? claudeCaps
        : launchProviderKind === "codex"
          ? codexCaps
          : opencodeCaps;
    if (caps === null) void refreshCaps(launchProviderKind);
  }, [open, launchProviderKind, claudeCaps, codexCaps, opencodeCaps, refreshCaps]);

  // Resolve the model selection when the dialog opens, when the chosen
  // agent changes, or once the preset list first loads. Deliberately
  // keyed on `selectedAgentId` + `presetsLoaded` (primitives) rather
  // than the `selectedAgent` object: a mid-dialog `presets` refresh
  // (e.g. switching project) must NOT clobber the user's in-dialog pick.
  const presetsLoaded = presets.length > 0;
  useEffect(() => {
    if (!open || !presetsLoaded) return;
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

  const handleSubmit = useCallback(async () => {
    if (!projectDir) return;

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
      userPrompt = buildPromptWithIssueContext(userPrompt, linkedIssue, issueBody);
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
          toast.info(
            linkedIssue
              ? `Issue #${linkedIssue.number} already has a workspace — switched to it.`
              : `"${openExistingBranch}" already has a workspace — switched to it.`,
          );
          await activateWorkspace(existingWsId);
          removePendingWorkspace(tempId);
          return;
        }

        let wsId: string;
        let agentHandled = false;
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

        const isDefaultOpen = openExistingBranch === "main" || openExistingBranch === "master";
        if (orphan) {
          wsId = await importWorktreeWorkspace(orphan.path, openExistingBranch, "single");
        } else if (isDefaultOpen) {
          // Open on the default branch always attaches to the real repo root.
          // The sidebar label will reflect actual HEAD via the live refresh loop,
          // so the user sees reality. No phantom worktree is created.
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
            null,
            launchSelection,
          );
          agentHandled = true;
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

        const pName = basename(projectDir);
        dbAddRecentProject(projectDir, pName).catch(console.error);
        if (linkedIssue) {
          try {
            await linkWorkspaceIssue(wsId, linkedIssue.number);
          } catch (linkErr) {
            console.error("Failed to link issue:", linkErr);
            toast.warning("Workspace created but issue linking failed. You can re-link from the workspace.");
          }
        }
        if (hostId !== null) {
          try {
            await setWorkspaceHost(wsId, hostId);
          } catch (hostErr) {
            console.error("Failed to set workspace host:", hostErr);
          }
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
        await activateWorkspace(existingWsId);
        removePendingWorkspace(tempId);
        return;
      }

      let wsId: string;
      let agentHandled = false;

      // Existing default branch (main/master): always attach to the real repo
      // root regardless of what the main repo currently has checked out. The
      // sidebar branch label reflects actual HEAD via the live refresh loop,
      // so the user sees reality instead of a phantom worktree at
      // ~/.codemux/worktrees/<project>/main. Feature branches always get a
      // proper worktree.
      const isDefault = resolvedBranch === "main" || resolvedBranch === "master";
      if (isDefault && !isNewBranch) {
        wsId = await createWorkspace(projectDir);
      } else {
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
            null,
            launchSelection,
          );
          agentHandled = true;
        }
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
      const pName =
        basename(projectDir);
      dbAddRecentProject(projectDir, pName).catch(console.error);

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

      removePendingWorkspace(tempId);
      await activateWorkspace(wsId);
    } catch (err) {
      toast.error(String(err));
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
    launchFamily,
    modelSelection,
    effectiveReasoning,
    effectiveContext,
    claudeCaps,
    setLastModelSelection,
    baseBranch,
    allBranches,
    branchMode,
    openExistingBranch,
    hostId,
    branchWorkspaceMap,
    worktrees,
    existingWorktreePaths,
    currentBranch,
    linkedIssue,
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
        showCloseButton={false}
        className="sm:max-w-2xl max-h-[min(70vh,600px)] !top-[calc(50%-min(35vh,300px))] !-translate-y-0 bg-popover p-0 gap-0 overflow-visible"
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
              onPaste={handlePasteImage}
              placeholder="What do you want to do?"
              className="min-h-24 max-h-48 resize-none border-0 bg-transparent dark:bg-transparent shadow-none focus-visible:ring-0 text-sm px-4 pt-3 pb-1"
              rows={1}
            />

            {/* Attachment chips + linked issue chip */}
            {(attachments.length > 0 || linkedIssue) && (
              <div className="flex flex-wrap gap-2 px-4 pb-2.5">
                {/* Linked issue chip */}
                {linkedIssue && (
                  <span
                    className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 py-0.5 pl-1 pr-1 text-[11px] text-foreground"
                    title={`#${linkedIssue.number} ${linkedIssue.title}`}
                  >
                    <span
                      className={cn(
                        "flex size-5 shrink-0 items-center justify-center rounded",
                        linkedIssue.state === "Open"
                          ? "bg-success/15 text-success"
                          : "bg-foreground/10 text-muted-foreground",
                      )}
                    >
                      <CircleDot className="h-3 w-3" />
                    </span>
                    <span className="font-mono tabular-nums text-muted-foreground">
                      #{linkedIssue.number}
                    </span>
                    <span className="max-w-[160px] truncate">{linkedIssue.title}</span>
                    <button
                      type="button"
                      aria-label={`Remove issue #${linkedIssue.number}`}
                      className="ml-0.5 rounded-full p-0.5 text-muted-foreground/70 transition-colors hover:bg-foreground/10 hover:text-foreground"
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
              <div className="mx-3 mb-2 flex items-center justify-between gap-2 rounded-md border border-border bg-muted/40 px-3 py-1.5 text-[11px] text-muted-foreground">
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
                  className="shrink-0 rounded-md border border-border bg-background px-2 py-0.5 font-medium text-foreground transition-colors hover:bg-muted"
                >
                  Open it
                </button>
              </div>
            )}

            {/* Footer inside textarea border */}
            <div className="flex items-center justify-between px-3 pb-3 pt-0">
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
                      className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
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
                      <Paperclip className="h-4 w-4" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="top">Attach files</TooltipContent>
                </Tooltip>

                {/* Link pull request */}
                {ghAvailable && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        aria-label="Link pull request"
                        className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
                        onClick={() => {
                          setIssuePickerOpen(false);
                          setPrPickerOpen(true);
                        }}
                      >
                        <GitPullRequest className="h-4 w-4" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent side="top">Link pull request</TooltipContent>
                  </Tooltip>
                )}

                {/* Link issue */}
                {isGithubRepo && !linkedIssue && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        aria-label="Link issue"
                        className="inline-flex size-7 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground outline-none"
                        onClick={() => {
                          setPrPickerOpen(false);
                          setIssuePickerOpen(true);
                        }}
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

          {/* PR picker — same floating treatment as the issue picker */}
          {prPickerOpen && projectDir && (
            <div
              ref={prPickerRef}
              className="absolute right-0 top-full mt-1 z-50 w-[320px] rounded-lg border border-border bg-popover shadow-lg overflow-hidden animate-in fade-in-0 zoom-in-95 duration-150"
            >
              <PrPickerPanel
                projectPath={projectDir}
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

          {/* Base branch picker */}
          {isGitRepo !== false && (
            <BranchPicker
              baseBranch={openExistingBranch || baseBranch}
              branches={detailedBranches}
              worktrees={worktrees}
              branchWorkspaceMap={branchWorkspaceMap}
              prBranches={prBranches}
              currentBranch={currentBranch}
              defaultBranchName={detectedDefaultBranch}
              loading={branchesLoading}
              onSelectBase={(branch) => {
                userPickedBaseRef.current = true;
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
