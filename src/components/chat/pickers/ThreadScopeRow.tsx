import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Check,
  ChevronDown,
  Folder,
  FolderPlus,
  GitBranch,
  GitFork,
  Globe,
  Home,
  Sparkle,
} from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { useProjectActions } from "@/hooks/use-project-actions";
import { basename } from "@/lib/path";
import { cn } from "@/lib/utils";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import type { DraftTarget } from "@/stores/chat-draft-store";
import {
  checkIsGitRepo,
  dbGetUiState,
  listBranchesDetailed,
} from "@/tauri/commands";
import type { BranchDetail, WorkspaceSnapshot } from "@/tauri/types";

import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

// Module-scoped stable empty array — returning a fresh `[]` literal
// from a Zustand selector triggers React's "getSnapshot should be
// cached" warning and re-render loops.
const EMPTY_WORKSPACES: WorkspaceSnapshot[] = [];

const GHOST_BTN =
  "inline-flex h-[27px] shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-semibold text-foreground/90 outline-none transition-colors hover:bg-foreground/[0.07] disabled:cursor-not-allowed disabled:opacity-50";

function formatRelativeTime(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unixSeconds;
  if (diff < 60) return "now";
  const mins = Math.floor(diff / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  const years = Math.floor(months / 12);
  return `${years}y`;
}

/** How the location control binds to its surface. One component, two
 *  consumers:
 *
 *  - `"draft"` — `DraftChatSurface`: locations retarget the client-side
 *    draft (`updateDraftTarget`); nothing is created until first send.
 *  - `"workspace"` — `AgentChatPane`'s new-thread empty state: the pane
 *    is bound to a REAL workspace, so locations navigate between
 *    existing workspaces (or open the new-workspace dialog) instead of
 *    mutating a draft. The location popover never creates hidden
 *    workspaces: "Home directory (~)" only renders when a home-rooted
 *    workspace already exists (or the pane itself is home-rooted), and
 *    picking a project fires `onSelectProject`, whose call site keeps
 *    the old ProjectPicker onChange behavior (activate the project's
 *    first workspace, else open the new-workspace dialog). */
export type ThreadScopeLocation =
  | {
      kind: "draft";
      target: DraftTarget;
      onChangeTarget: (target: DraftTarget) => void;
    }
  | {
      kind: "workspace";
      /** True when the pane's workspace is home-rooted
       *  (`project_root === $HOME`). */
      isHome: boolean;
      /** Activate an EXISTING home-rooted workspace. Only ever called
       *  with a workspace id that is already in the app-state. */
      onSelectHomeWorkspace: (workspaceId: string) => void;
      /** The user picked a different project (from the list or via
       *  "Open another project…"). Call-site decides activate-vs-dialog. */
      onSelectProject: (projectPath: string) => void;
    };

export interface ThreadScopeRowProps {
  /** Location-control binding — see `ThreadScopeLocation`. */
  location: ThreadScopeLocation;
  /** Resolved project root the checkout + branch controls scope to,
   *  or `null` when there is no project (home draft / home-rooted
   *  pane, or an `existing_workspace` draft target whose workspace
   *  hasn't hydrated into app-state yet) — the row then shows only
   *  the location control. */
  projectPath: string | null;
  checkoutMode: "current" | "worktree";
  worktreeName: string;
  baseBranch: string;
  disabled?: boolean;
  onChangeCheckoutMode: (mode: "current" | "worktree") => void;
  onChangeWorktreeName: (name: string) => void;
  onChangeBaseBranch: (branch: string) => void;
  /** Rendered after `BranchControl`, right-aligned — `AgentChatPane`
   *  passes `<WorkspaceStatusCluster />` here so the empty-thread row
   *  carries the same passive git/PR status as the running-thread
   *  Context Row (`docs/features/agent-chat.md` "Context Row"). Omitted
   *  by `DraftChatSurface`, which has no real workspace yet. */
  trailing?: ReactNode;
}

/**
 * Thread Scope redesign — the borderless scope row rendered BELOW the
 * composer (`Composer`'s `belowComposerSlot`): location · checkout on
 * the left, "from ⑂ branch" on the right, and a centered muted hint
 * underneath spelling out what a first send will do.
 *
 * Rendered by BOTH first-send surfaces (see `ThreadScopeLocation`):
 * `DraftChatSurface` and `AgentChatPane`'s new-thread empty state.
 * Replaces the old above-composer `WorktreePicker` +
 * `DerivativeBranchPicker` pill pair and the home-state
 * `ProjectPicker` those surfaces used to put in `zone1Override`.
 */
export function ThreadScopeRow({
  location,
  projectPath,
  checkoutMode,
  worktreeName,
  baseBranch,
  disabled,
  onChangeCheckoutMode,
  onChangeWorktreeName,
  onChangeBaseBranch,
  trailing,
}: ThreadScopeRowProps) {
  const isHome =
    location.kind === "draft"
      ? location.target.kind === "home"
      : location.isHome;
  const showProjectControls = !isHome && projectPath !== null;

  // Non-git projects can't have worktrees or base branches — hide the
  // checkout + branch controls instead of letting a "New worktree" send
  // die on the backend's `Not a git repository` error.
  //
  // Two-tier source for "is this project a git repo?":
  //  1. Snapshot first — the matching workspace row's `is_git` flag
  //     (optimistic `true` when the flag is unknown/undefined). This is
  //     the common case: most targeted projects already have a live
  //     workspace carrying the flag.
  //  2. Async probe fallback — when NO workspace row matches
  //     `projectPath` (a fresh draft targeting a just-opened project
  //     before app-state hydrates, or a recent project with no live
  //     workspace), the snapshot can't answer, so fall back to a live
  //     `check_is_git_repo` probe cached per path. While the probe is in
  //     flight (or if it returns a non-boolean — the dev mock's default
  //     fallback, or an IPC failure) we stay optimistic `true` so the
  //     controls never flash off.
  const workspaces = useAppStore(
    (s) => s.appState?.workspaces ?? EMPTY_WORKSPACES,
  );
  const matchedWorkspace = useMemo(() => {
    if (!projectPath) return undefined;
    return workspaces.find((w) => (w.project_root ?? w.cwd) === projectPath);
  }, [workspaces, projectPath]);

  // Per-path probe cache for projects with no live workspace row.
  const [gitProbe, setGitProbe] = useState<Record<string, boolean>>({});
  useEffect(() => {
    // Only probe when a project is targeted AND the snapshot can't answer
    // (no workspace row matches). Skip once we already have a result.
    if (!projectPath || matchedWorkspace) return;
    if (projectPath in gitProbe) return;
    let cancelled = false;
    checkIsGitRepo(projectPath)
      .then((res) => {
        if (cancelled) return;
        // A non-boolean (null/undefined from the dev mock's default
        // fallback) stays optimistic-true, matching the snapshot default.
        setGitProbe((prev) => ({
          ...prev,
          [projectPath]: typeof res === "boolean" ? res : true,
        }));
      })
      .catch(() => {
        if (!cancelled) {
          setGitProbe((prev) => ({ ...prev, [projectPath]: true }));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath, matchedWorkspace, gitProbe]);

  const projectIsGit = useMemo(() => {
    if (!projectPath) return true;
    if (matchedWorkspace) return matchedWorkspace.is_git !== false;
    // No workspace row — defer to the probe cache, staying optimistic
    // (true) until it resolves.
    const probed = gitProbe[projectPath];
    return probed === undefined ? true : probed;
  }, [projectPath, matchedWorkspace, gitProbe]);

  // A stale draft (or a project switch) can arrive with "worktree" mode
  // already picked — snap it back so first send can't hit the error.
  useEffect(() => {
    if (!projectIsGit && checkoutMode === "worktree") {
      onChangeCheckoutMode("current");
    }
  }, [projectIsGit, checkoutMode, onChangeCheckoutMode]);

  return (
    <div className="rise-in flex flex-col items-center gap-3 px-1">
      <div className="flex w-full items-center justify-between gap-2.5">
        <div className="flex min-w-0 items-center gap-0.5">
          <LocationControl
            location={location}
            isHome={isHome}
            activeProjectPath={projectPath}
            disabled={disabled}
          />
          {showProjectControls && projectIsGit && (
            <>
              <span className="select-none text-muted-foreground/50">·</span>
              <CheckoutControl
                checkoutMode={checkoutMode}
                worktreeName={worktreeName}
                disabled={disabled}
                onChangeCheckoutMode={onChangeCheckoutMode}
                onChangeWorktreeName={onChangeWorktreeName}
              />
            </>
          )}
        </div>
        {(showProjectControls || trailing) && (
          <div className="flex shrink-0 items-center gap-1.5">
            {showProjectControls && projectIsGit && projectPath && (
              <BranchControl
                projectPath={projectPath}
                checkoutMode={checkoutMode}
                baseBranch={baseBranch}
                disabled={disabled}
                onChangeCheckoutMode={onChangeCheckoutMode}
                onChangeBaseBranch={onChangeBaseBranch}
              />
            )}
            {trailing}
          </div>
        )}
      </div>
    </div>
  );
}

// ── Location control ──

interface ProjectAvatarState {
  color: string | null;
  image: string | null;
  imageVersion: string | null;
}
const EMPTY_AVATAR: ProjectAvatarState = { color: null, image: null, imageVersion: null };

function LocationControl({
  location,
  isHome,
  activeProjectPath,
  disabled,
}: {
  location: ThreadScopeLocation;
  isHome: boolean;
  /** The currently-active project root (`null` when home / unresolved). */
  activeProjectPath: string | null;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [projectAvatars, setProjectAvatars] = useState<
    Record<string, ProjectAvatarState>
  >({});

  const homeDir = useHomeDir();
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? EMPTY_WORKSPACES);
  const hosts = useHosts();
  const groups = useProjectGroupedWorkspaces(workspaces, homeDir, hosts);
  const { openProject } = useProjectActions();

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    (async () => {
      const entries = await Promise.all(
        groups.map(async (g) => {
          const [color, image, imageVersion] = await Promise.all([
            dbGetUiState(`project.color:${g.projectPath}`).catch(() => null),
            dbGetUiState(`project.image:${g.projectPath}`).catch(() => null),
            dbGetUiState(`project.image.v:${g.projectPath}`).catch(() => null),
          ]);
          return [
            g.projectPath,
            { color: color || null, image: image || null, imageVersion: imageVersion || null },
          ] as const;
        }),
      );
      if (cancelled) return;
      setProjectAvatars(Object.fromEntries(entries));
    })();
    return () => {
      cancelled = true;
    };
  }, [open, groups]);

  // Workspace mode — first existing home-rooted workspace, if any.
  // Drives both the Home row's visibility (never offer a location we
  // would have to create a workspace for) and its click target.
  const firstHomeWorkspaceId = useMemo(() => {
    if (homeDir === null) return null;
    const homeGroup = groups.find((g) => g.projectPath === homeDir);
    return homeGroup?.workspaces[0]?.workspace_id ?? null;
  }, [groups, homeDir]);

  const showHomeOption =
    location.kind === "draft" || isHome || firstHomeWorkspaceId !== null;

  const handleSelectHome = () => {
    setOpen(false);
    if (location.kind === "draft") {
      location.onChangeTarget({ kind: "home" });
      return;
    }
    // Workspace mode: already home → no-op; otherwise hop to the
    // existing home-rooted workspace (the row only renders when one
    // exists — see `showHomeOption`).
    if (location.isHome) return;
    if (firstHomeWorkspaceId) {
      location.onSelectHomeWorkspace(firstHomeWorkspaceId);
    }
  };

  const handleSelectProject = (targetProjectPath: string) => {
    setOpen(false);
    if (location.kind === "draft") {
      location.onChangeTarget({
        kind: "project",
        projectPath: targetProjectPath,
      });
      return;
    }
    // Workspace mode: picking the already-active project is a no-op.
    if (targetProjectPath === activeProjectPath) return;
    location.onSelectProject(targetProjectPath);
  };

  const label = isHome
    ? "Home"
    : activeProjectPath
      ? basename(activeProjectPath)
      : "Project";

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" disabled={disabled} className={GHOST_BTN}>
          {isHome ? (
            <Home className="size-3.5 text-status-remote" />
          ) : (
            <Folder className="size-3.5 text-muted-foreground" />
          )}
          <span className="max-w-[140px] truncate">{label}</span>
          <ChevronDown className="size-2.5 opacity-45" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[250px] p-1.5"
        align="start"
        side="top"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <div className="px-2 pb-1 pt-1 font-mono text-[9.5px] font-semibold uppercase tracking-wide text-muted-foreground">
          Run in
        </div>
        {showHomeOption && (
          <button
            type="button"
            onClick={handleSelectHome}
            className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-xs font-medium text-foreground transition-colors hover:bg-foreground/[0.08]"
          >
            <span className="flex size-5 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
              <Home className="size-3" />
            </span>
            <span className="min-w-0 flex-1 truncate">Home directory (~)</span>
            {isHome && (
              <Check className="size-3.5 shrink-0 text-accent-ember" />
            )}
          </button>
        )}
        {groups
          .filter((g) => g.projectPath !== homeDir)
          .map((g) => {
            const active = g.projectPath === activeProjectPath;
            const avatar = projectAvatars[g.projectPath] ?? EMPTY_AVATAR;
            return (
              <button
                key={g.projectPath}
                type="button"
                onClick={() => handleSelectProject(g.projectPath)}
                className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-xs font-medium text-foreground transition-colors hover:bg-foreground/[0.08]"
              >
                <ProjectAvatar
                  name={g.projectName}
                  color={avatar.color}
                  imageUrl={avatar.image}
                  cacheBust={avatar.imageVersion}
                  size="md"
                  shape="square"
                />
                <span className="min-w-0 flex-1 truncate">{g.projectName}</span>
                {active && <Check className="size-3.5 shrink-0 text-accent-ember" />}
              </button>
            );
          })}
        <div className="my-1 h-px bg-border" />
        <button
          type="button"
          onClick={async () => {
            setOpen(false);
            const result = await openProject();
            if (result.success && result.path) {
              handleSelectProject(result.path);
            }
          }}
          className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[11.5px] text-muted-foreground transition-colors hover:bg-foreground/[0.08] hover:text-foreground"
        >
          <FolderPlus className="size-3.5" />
          Open another project…
        </button>
      </PopoverContent>
    </Popover>
  );
}

// ── Checkout control ──

function CheckoutControl({
  checkoutMode,
  worktreeName,
  disabled,
  onChangeCheckoutMode,
  onChangeWorktreeName,
}: {
  checkoutMode: "current" | "worktree";
  worktreeName: string;
  disabled?: boolean;
  onChangeCheckoutMode: (mode: "current" | "worktree") => void;
  onChangeWorktreeName: (name: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const isWorktree = checkoutMode === "worktree";
  const isCurrent = !isWorktree;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" disabled={disabled} className={GHOST_BTN}>
          {isWorktree ? (
            <GitFork className="size-3 text-muted-foreground" />
          ) : (
            <Folder className="size-3 text-muted-foreground" />
          )}
          {isWorktree ? "New worktree" : "Current checkout"}
          <ChevronDown className="size-2.5 opacity-45" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[320px] p-1.5"
        align="start"
        side="top"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <div className="px-2 pb-1 pt-1 font-mono text-[9.5px] font-semibold uppercase tracking-wide text-muted-foreground">
          Where should the agent work?
        </div>
        <button
          type="button"
          onClick={() => onChangeCheckoutMode("current")}
          className="flex w-full items-start gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-foreground/[0.08]"
        >
          <Folder className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="text-xs font-semibold text-foreground">
                Current checkout
              </span>
              {isCurrent && (
                <Check className="size-3.5 shrink-0 text-accent-ember" />
              )}
            </span>
            <span className="mt-0.5 block text-[11px] leading-snug text-muted-foreground">
              Work directly in the project's checked-out branch. No new
              worktree.
            </span>
          </span>
        </button>
        <button
          type="button"
          onClick={() => onChangeCheckoutMode("worktree")}
          className="flex w-full items-start gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors hover:bg-foreground/[0.08]"
        >
          <GitFork className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="text-xs font-semibold text-foreground">
                New worktree
              </span>
              {isWorktree && (
                <Check className="size-3.5 shrink-0 text-accent-ember" />
              )}
            </span>
            <span className="mt-0.5 block text-[11px] leading-snug text-muted-foreground">
              Branch off in an isolated worktree, so the agent can't touch
              your working copy.
            </span>
          </span>
        </button>
        {isWorktree && (
          <div className="px-1 pb-0.5 pt-1">
            <div className="flex h-8 items-center gap-2 rounded-lg border border-border bg-background px-2.5">
              <GitFork className="size-3 shrink-0 text-muted-foreground" />
              <input
                value={worktreeName}
                onChange={(e) => onChangeWorktreeName(e.target.value)}
                onClick={(e) => e.stopPropagation()}
                onKeyDown={(e) => e.stopPropagation()}
                placeholder="name — leave empty to auto-name"
                aria-label="Worktree name"
                className="flex-1 min-w-0 bg-transparent font-mono text-[11.5px] text-foreground outline-none placeholder:text-muted-foreground/60"
              />
            </div>
            <div className="mt-1.5 flex items-start gap-1.5 px-0.5 text-[10.5px] leading-snug text-muted-foreground">
              <Sparkle className="mt-px size-3 shrink-0 text-status-remote" />
              <span>
                Empty → CodeMux names it from your first message, like the
                CLI does.
              </span>
            </div>
          </div>
        )}
      </PopoverContent>
    </Popover>
  );
}

// ── Branch control ──

type FilterMode = "all" | "worktrees";

function BranchControl({
  projectPath,
  checkoutMode,
  baseBranch,
  disabled,
  onChangeCheckoutMode,
  onChangeBaseBranch,
}: {
  projectPath: string;
  checkoutMode: "current" | "worktree";
  baseBranch: string;
  disabled?: boolean;
  onChangeCheckoutMode: (mode: "current" | "worktree") => void;
  onChangeBaseBranch: (branch: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [branches, setBranches] = useState<BranchDetail[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [filterMode, setFilterMode] = useState<FilterMode>("all");

  const homeDir = useHomeDir();
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? EMPTY_WORKSPACES);
  const groups = useProjectGroupedWorkspaces(workspaces, homeDir);
  const worktreeBranches = useMemo(() => {
    const set = new Set<string>();
    const group = groups.find((g) => g.projectPath === projectPath);
    if (!group) return set;
    for (const ws of group.workspaces) {
      if (ws.git_branch) set.add(ws.git_branch);
    }
    return set;
  }, [groups, projectPath]);

  // Fetch eagerly on mount (not just on open) so the current/worktree
  // mode's displayed branch corrects itself before the user opens the
  // popover (carried over from the retired DerivativeBranchPicker).
  useEffect(() => {
    if (!projectPath) return;
    if (branches !== null) return;
    let cancelled = false;
    setLoading(true);
    listBranchesDetailed(projectPath)
      .then((rows) => {
        if (!cancelled) setBranches(rows);
      })
      .catch(() => {
        if (!cancelled) setBranches([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath, branches]);

  // Seed / auto-correct the base branch once the branch list loads —
  // main/master/first-branch heuristic (carried over from the retired
  // DerivativeBranchPicker), used when the caller couldn't seed the
  // value from the workspace snapshot's `git_branch`.
  useEffect(() => {
    if (!branches || branches.length === 0) return;
    if (baseBranch && branches.some((b) => b.name === baseBranch)) return;
    const names = branches.map((b) => b.name);
    const best = names.includes("main")
      ? "main"
      : names.includes("master")
        ? "master"
        : names[0];
    onChangeBaseBranch(best);
  }, [branches, baseBranch, onChangeBaseBranch]);

  const handleSelect = (name: string) => {
    setOpen(false);
    // Picking a DIFFERENT branch while on "current checkout" can't
    // silently repoint the user's real checkout — it means "branch
    // off from there instead," so flip to a deferred worktree with
    // the picked branch as its base.
    if (checkoutMode === "current" && name !== baseBranch) {
      onChangeCheckoutMode("worktree");
    }
    onChangeBaseBranch(name);
  };

  const handleOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) setFilterMode("all");
  };

  const allCount = branches?.length ?? 0;
  const worktreeCount = useMemo(() => {
    if (!branches) return 0;
    return branches.filter((b) => worktreeBranches.has(b.name)).length;
  }, [branches, worktreeBranches]);
  const visibleBranches = useMemo(() => {
    if (!branches) return [] as BranchDetail[];
    if (filterMode === "all") return branches;
    return branches.filter((b) => worktreeBranches.has(b.name));
  }, [branches, filterMode, worktreeBranches]);

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <button type="button" disabled={disabled} className={GHOST_BTN}>
          <span className="font-medium text-muted-foreground">from</span>
          <GitBranch className="size-3 text-muted-foreground" />
          <span className="max-w-[160px] truncate font-mono text-foreground">
            {baseBranch || "…"}
          </span>
          <ChevronDown className="size-2.5 opacity-45" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[340px] p-0"
        align="end"
        side="top"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command>
          <CommandInput placeholder="Search branches…" className="h-8" />
          <div className="mx-2 mt-1 mb-1 flex items-center gap-0.5 rounded-md bg-muted/40 p-0.5">
            <button
              type="button"
              className={cn(
                "flex-1 rounded-md px-2 py-1 text-xs transition-colors",
                filterMode === "all"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("all")}
            >
              All <span className="text-[10px] opacity-60">{allCount}</span>
            </button>
            <button
              type="button"
              className={cn(
                "flex-1 rounded-md px-2 py-1 text-xs transition-colors",
                filterMode === "worktrees"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setFilterMode("worktrees")}
            >
              Worktrees{" "}
              <span className="text-[10px] opacity-60">{worktreeCount}</span>
            </button>
          </div>
          <CommandList
            className="max-h-[280px] overflow-y-auto [scrollbar-width:thin]"
            onWheel={(e) => e.stopPropagation()}
          >
            <CommandEmpty>
              {loading
                ? "Loading…"
                : filterMode === "worktrees"
                  ? "No active worktrees"
                  : "No branches"}
            </CommandEmpty>
            {visibleBranches.length > 0 && (
              <CommandGroup>
                {visibleBranches.map((branch) => {
                  const hasWorktree = worktreeBranches.has(branch.name);
                  const active = branch.name === baseBranch;
                  return (
                    <CommandItem
                      key={branch.name}
                      value={branch.name}
                      onSelect={() => handleSelect(branch.name)}
                      className="h-8 gap-2 px-2 text-xs"
                      data-checked={active ? "true" : undefined}
                    >
                      <BranchRowIcon
                        hasWorktree={hasWorktree}
                        isLocal={branch.is_local}
                        isRemote={branch.is_remote}
                      />
                      <span className="min-w-0 flex-1 truncate font-mono">
                        {branch.name}
                      </span>
                      {hasWorktree && (
                        <span className="shrink-0 rounded bg-status-working/15 px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wide text-status-working">
                          WORKTREE
                        </span>
                      )}
                      <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
                        {formatRelativeTime(branch.last_commit_unix)}
                      </span>
                    </CommandItem>
                  );
                })}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function BranchRowIcon({
  hasWorktree,
  isLocal,
  isRemote,
}: {
  hasWorktree: boolean;
  isLocal: boolean;
  isRemote: boolean;
}) {
  if (hasWorktree) {
    return <GitFork className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (isRemote) {
    return <Globe className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  if (isLocal) {
    return <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  return <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />;
}
