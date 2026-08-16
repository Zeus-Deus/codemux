/**
 * Handing a pull-request problem to an agent.
 *
 * Three surfaces need this: a failing check, a review comment, and a
 * conflict with the base branch. They differ only in what the agent is
 * told — where the thread opens, how the prompt gets there and what the
 * user is told afterwards is one path, written once here.
 *
 * The prompts are deliberately verbose. The agent lands in a fresh
 * thread knowing nothing: not the pull request, not the failure, not
 * which branch it is standing on. Everything it would otherwise have to
 * go and fetch is in the first message, quoted from data this panel
 * already has.
 */

import {
  applyPreset,
  createTab,
  createWorktreeWorkspaceResult,
  getCheckLogExcerpt,
  getPresets,
} from "@/tauri/commands";
import type { ModelSelection, TerminalPreset, WorkspaceSnapshot } from "@/tauri/types";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { toast } from "@/lib/toast";

// ── Task shapes ───────────────────────────────────────────────────────

export type HandoffKind = "failing-check" | "review-thread" | "conflicts";

/** The failing check, plus whatever of its log we already had. */
export interface FailingCheckTask {
  kind: "failing-check";
  checkName: string;
  /** Already-fetched excerpt. Absent ⇒ fetched during the handoff. */
  logExcerpt?: string | null;
  detailUrl?: string | null;
}

/** One review comment, with its anchor and the thread it belongs to. */
export interface ReviewThreadTask {
  kind: "review-thread";
  reviewer: string;
  body: string;
  /** File anchor, when the comment is an inline one. */
  path?: string | null;
  line?: number | null;
  /** APPROVED / CHANGES_REQUESTED / COMMENTED, when known. */
  verdict?: string | null;
  /** The comment this one replies to, so a reply reads in context. */
  parent?: { author: string; body: string } | null;
}

export interface ConflictsTask {
  kind: "conflicts";
  /** Only passed when already known for free — never probed for. */
  files?: string[];
}

export type HandoffTask = FailingCheckTask | ReviewThreadTask | ConflictsTask;

/** The pull-request fields a prompt quotes. */
export interface HandoffPr {
  number: number;
  title: string;
  url: string;
  head_branch: string | null;
  base_branch: string | null;
}

export interface HandoffRequest {
  pr: HandoffPr;
  task: HandoffTask;
  /** `owner/repo#285`, or `#285` when the slug is unknown. */
  prRef: string;
  /** Repository root — where a worktree would be cut. */
  projectRoot: string;
  /** The checkout being read; used to fetch a missing log excerpt. */
  cwd: string;
  /**
   * Set only when this workspace is standing on the PR's head branch.
   * Then the branch is already here and no worktree is needed.
   */
  currentWorkspaceId?: string | null;
  /** Host CLI (`gh`, `glab`) — named in the prompt only if it is real. */
  cli?: string | null;
  providerKind?: string | null;
}

export type HandoffRoute = "current" | "existing" | "worktree" | "adopted";

export interface HandoffOutcome {
  route: HandoffRoute;
  workspaceId: string;
  /** Workspace the thread landed in, for the toast. */
  workspaceTitle: string;
  presetName: string;
  prompt: string;
}

// ── Preset resolution ─────────────────────────────────────────────────

/**
 * Same fallback the new-workspace dialog uses, kept in one place.
 *
 * Candidates are the pinned CLI presets: chat-agent presets can't be
 * launched through `apply_preset` at all, and an unpinned preset is one
 * the user has taken off their own launcher.
 */
const FALLBACK_PRESET_ID = "builtin-claude";

export function pickAgentPreset(
  presets: TerminalPreset[],
  lastSelectedAgentId: string | null,
): TerminalPreset | null {
  const candidates = presets.filter((p) => p.pinned && p.kind === "cli");
  const remembered = lastSelectedAgentId
    ? candidates.find((p) => p.id === lastSelectedAgentId)
    : undefined;
  return (
    remembered ??
    candidates.find((p) => p.id === FALLBACK_PRESET_ID) ??
    candidates[0] ??
    null
  );
}

async function resolveAgentPreset(): Promise<{
  presetId: string;
  presetName: string;
  modelSelection: ModelSelection | null;
}> {
  let preset: TerminalPreset | null = null;
  try {
    const snapshot = await getPresets();
    preset = pickAgentPreset(
      snapshot.presets,
      useUIStore.getState().lastSelectedAgentId,
    );
  } catch {
    // A preset store that won't load is not a reason to drop the
    // handoff — the built-in id is the same one the app ships with.
    preset = null;
  }
  return {
    presetId: preset?.id ?? FALLBACK_PRESET_ID,
    presetName: preset?.name ?? "Agent",
    modelSelection: preset?.launch_config?.model_selection ?? null,
  };
}

// ── Prompt composition ────────────────────────────────────────────────

/** ``` fences that survive a log excerpt containing its own fence. */
function fence(body: string): string {
  const longest = [...body.matchAll(/`{3,}/g)].reduce(
    (max, m) => Math.max(max, m[0].length),
    2,
  );
  const rail = "`".repeat(longest + 1);
  return `${rail}\n${body}\n${rail}`;
}

function quote(body: string): string {
  return body
    .trim()
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");
}

/** The two lines every prompt opens with: which PR, which branches. */
function header(pr: HandoffPr, prRef: string): string {
  const base = pr.base_branch ?? "the base branch";
  const branchLine = pr.head_branch
    ? `Branch \`${pr.head_branch}\` → \`${base}\`.`
    : `Base branch \`${base}\`.`;
  return `${prRef} — "${pr.title}"\n${pr.url}\n${branchLine}`;
}

/**
 * How to verify, in the words the repo itself uses.
 *
 * Deliberately not a command: the right check depends on what gets
 * touched, and this project's own instructions say to run the smallest
 * relevant one rather than the full suite.
 */
const VERIFY_STEP =
  "Verify with the smallest relevant check for what you touched — follow the repo's AGENTS.md (or CONTRIBUTING) for the exact commands instead of running the whole suite.";

function pushStep(pr: HandoffPr): string {
  return pr.head_branch
    ? `Commit and push to \`${pr.head_branch}\` so the pull request updates.`
    : "Commit and push the branch so the pull request updates.";
}

/**
 * What the agent may do on the host afterwards.
 *
 * Codemux has no reply-to-thread or resolve-thread command, so the
 * prompt does not pretend it does. What it does have is a signed-in host
 * CLI in this very shell, and `gh` has a documented comment subcommand —
 * so that one is spelled out, and every other host is named without
 * inventing its syntax.
 */
function hostReplyStep(pr: HandoffPr, cli: string | null | undefined, kind: string | null | undefined): string | null {
  if (!cli) return null;
  if (kind === "github" || cli === "gh") {
    return `Optional: once it is pushed, say so in the pull-request conversation with \`gh pr comment ${pr.number} --body "…"\`. Nothing here can resolve the thread itself — leave that to the reviewer.`;
  }
  return `Optional: once it is pushed, you can note the fix in the conversation with the \`${cli}\` CLI, which is installed and signed in in this shell. Nothing here can resolve the thread itself — leave that to the reviewer.`;
}

function numbered(steps: (string | null)[]): string {
  return steps
    .filter((s): s is string => !!s)
    .map((step, i) => `${i + 1}. ${step}`)
    .join("\n");
}

export function buildHandoffPrompt(req: HandoffRequest, excerpt?: string | null): string {
  const { pr, task, prRef } = req;
  const top = header(pr, prRef);

  if (task.kind === "failing-check") {
    const log = (excerpt ?? task.logExcerpt ?? "").trim();
    const parts = [
      top,
      `The check \`${task.checkName}\` is failing on this pull request.`,
      log ? `Failing log excerpt:\n${fence(log)}` : null,
      task.detailUrl ? `Full run: ${task.detailUrl}` : null,
      numbered([
        `Reproduce \`${task.checkName}\` locally on this branch — read the workflow that runs it if the command isn't obvious.`,
        "Fix the cause rather than the assertion.",
        VERIFY_STEP,
        pushStep(pr),
      ]),
    ];
    return parts.filter(Boolean).join("\n\n");
  }

  if (task.kind === "review-thread") {
    const anchor = task.path
      ? `${task.path}${task.line != null ? `:${task.line}` : ""}`
      : null;
    const verdict =
      task.verdict === "CHANGES_REQUESTED"
        ? "requested changes on"
        : task.verdict === "APPROVED"
          ? "approved, with a note on"
          : "commented on";
    const parts = [
      top,
      `${task.reviewer} ${verdict} this pull request${anchor ? `, on \`${anchor}\`` : ""}:`,
      task.parent
        ? `Earlier in the thread, ${task.parent.author} wrote:\n${quote(task.parent.body)}\n\n${task.reviewer} replied:\n${quote(task.body)}`
        : quote(task.body),
      numbered([
        anchor
          ? `Read \`${anchor}\` in context before changing anything.`
          : "Find what the comment refers to before changing anything.",
        "Address the feedback. If you disagree with it, say so in your answer rather than half-applying it.",
        VERIFY_STEP,
        pushStep(pr),
        hostReplyStep(pr, req.cli, req.providerKind),
      ]),
    ];
    return parts.filter(Boolean).join("\n\n");
  }

  const base = pr.base_branch ?? "the base branch";
  const parts = [
    top,
    `The host reports that this branch conflicts with \`${base}\`.`,
    task.files && task.files.length > 0
      ? `Conflicting files:\n${task.files.map((f) => `- ${f}`).join("\n")}`
      : null,
    numbered([
      `Bring \`${base}\` in (\`git fetch origin\` then merge or rebase, whichever this repo's history uses) and see the conflicts for yourself — the list above, if there is one, is the host's and may be stale.`,
      "Resolve each conflict by the intent of both sides, not by picking a side wholesale.",
      VERIFY_STEP,
      `Push the resolved branch${pr.head_branch ? ` to \`${pr.head_branch}\`` : ""}. If you rebased, push with \`--force-with-lease\` — never a bare \`--force\`.`,
    ]),
  ];
  return parts.filter(Boolean).join("\n\n");
}

/** The half-sentence a toast uses to say what was handed over. */
export function taskLabel(task: HandoffTask, pr: HandoffPr): string {
  if (task.kind === "failing-check") return `fixing ${task.checkName}`;
  if (task.kind === "review-thread") return `addressing ${task.reviewer}'s comment`;
  return `resolving conflicts with ${pr.base_branch ?? "the base branch"}`;
}

// ── Routing ───────────────────────────────────────────────────────────

function workspaceById(id: string): WorkspaceSnapshot | null {
  const list = useAppStore.getState().appState?.workspaces;
  return list?.find((w) => w.workspace_id === id) ?? null;
}

/** A workspace already standing on this branch, in this project. */
export function findWorkspaceForBranch(
  projectRoot: string,
  branch: string,
): WorkspaceSnapshot | null {
  const list = useAppStore.getState().appState?.workspaces;
  if (!list) return null;
  return (
    list.find((w) => w.project_root === projectRoot && w.git_branch === branch) ??
    null
  );
}

/**
 * Start the preset in a *fresh* tab of an existing workspace.
 *
 * `apply_preset` only carries an initial prompt on its `current_terminal`
 * path — the `new_tab` branch spawns the agent with no prompt at all. So
 * the tab is created first (which makes it the active one) and the
 * preset is then applied to it. That also means we never type into the
 * terminal the user was already working in.
 */
async function startThreadIn(
  workspaceId: string,
  presetId: string,
  prompt: string,
  modelSelection: ModelSelection | null,
): Promise<void> {
  await createTab(workspaceId, "terminal");
  await applyPreset(workspaceId, presetId, "current_terminal", prompt, modelSelection);
}

/**
 * Hand the task to an agent and tell the user where it went.
 *
 * Throws on failure so the initiating button can drop its in-flight
 * state and show the reason; the success toast is raised here, once.
 */
export async function handOffToAgent(req: HandoffRequest): Promise<HandoffOutcome> {
  const { pr, task, projectRoot, cwd, currentWorkspaceId } = req;

  // Only the check card can be missing its excerpt (the card fetches
  // lazily and may not have been expanded), and a prompt without the
  // failure in it is most of the value gone.
  let excerpt: string | null = null;
  if (task.kind === "failing-check" && !task.logExcerpt) {
    excerpt = await getCheckLogExcerpt(cwd, pr.number, task.checkName).catch(() => null);
  }

  const prompt = buildHandoffPrompt(req, excerpt);
  const { presetId, presetName, modelSelection } = await resolveAgentPreset();

  const finish = (route: HandoffRoute, workspaceId: string): HandoffOutcome => {
    const title = workspaceById(workspaceId)?.title ?? pr.head_branch ?? "a new workspace";
    const where =
      route === "current"
        ? "in this workspace"
        : route === "worktree"
          ? `in a new worktree for ${pr.head_branch}`
          : `in ${title}`;
    toast.success(`${presetName} is ${taskLabel(task, pr)} — new thread ${where}`);
    return { route, workspaceId, workspaceTitle: title, presetName, prompt };
  };

  // (a) You are standing in the branch: the worktree already exists and
  // it is the one on screen.
  if (currentWorkspaceId) {
    await startThreadIn(currentWorkspaceId, presetId, prompt, modelSelection);
    return finish("current", currentWorkspaceId);
  }

  if (!pr.head_branch) {
    throw new Error("This pull request has no head branch to check out.");
  }

  // (b) Some other workspace has the branch — go there rather than
  // cutting a second worktree for the same branch.
  const existing = findWorkspaceForBranch(projectRoot, pr.head_branch);
  if (existing) {
    await activateWorkspaceInteraction(existing.workspace_id);
    await startThreadIn(existing.workspace_id, presetId, prompt, modelSelection);
    return finish("existing", existing.workspace_id);
  }

  // (c) Nowhere to land: cut the worktree, with the prompt attached.
  const result = await createWorktreeWorkspaceResult(
    projectRoot,
    pr.head_branch,
    false,
    "single",
    null,
    prompt,
    presetId,
    pr.number,
    modelSelection,
  );

  // An adopted workspace is one the backend attached to instead of
  // creating, and adoption drops the prompt and the preset. Rather than
  // telling the user their instruction went nowhere, start the thread
  // the second way.
  if (result.adopted) {
    await startThreadIn(result.workspaceId, presetId, prompt, modelSelection);
    return finish("adopted", result.workspaceId);
  }

  return finish("worktree", result.workspaceId);
}
