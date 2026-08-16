/**
 * Opening a pull request from a branch an agent just wrote.
 *
 * The premise of this surface is that the form already has the answers.
 * Four commits exist in this workspace; between them they contain a
 * title, a description, and often the verification the reviewer is going
 * to ask about. An empty box would be asking the user to retype what is
 * sitting in `git log`.
 *
 * So the fields arrive filled in — and the moment the user touches one,
 * the claim that it was drafted from their commits comes off it. The
 * note is a statement about where the words came from, not decoration on
 * a text field.
 *
 * What isn't here, and why:
 *
 * - **Rewrite with agent.** A handoff opens a thread in another
 *   workspace and returns nothing to this form; there is no path by
 *   which an agent's rewrite could land back in these two fields. A chip
 *   that opened a thread and left the description untouched would be a
 *   lie about what the button does, so it is not drawn.
 * - **+ label.** `create_pull_request` takes no labels at any layer
 *   (path, title, body, base, draft), and neither adapter's create
 *   accepts them. Rather than a chip that collects labels and drops
 *   them, the chip is absent until the backend can carry them.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, Loader2, X } from "lucide-react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  createPullRequest,
  getGitBranchInfo,
  getGitStatus,
  gitCommitsAhead,
  gitPushChanges,
  listBranches,
  listDirectory,
  readFile,
  requestPrReview,
} from "@/tauri/commands";
import type { CommitSummary, PullRequestInfo } from "@/tauri/types";
import {
  draftBody,
  draftTitle,
  GITHUB_TEMPLATE_PATHS,
  GITLAB_TEMPLATE_DIR,
  joinPath,
  type DraftSource,
} from "@/lib/pr-draft";
import type { ProviderPresentation } from "@/lib/source-control";
import { toast } from "@/lib/toast";
import { btnCard, btnEmberSolid, plural } from "./review-ui";

/** Enough to describe any branch a person would open a pull request from;
 *  past this the description would be unreadable anyway. */
const MAX_COMMITS = 50;

/**
 * The repository's own pull-request template, if it has one.
 *
 * GitHub keeps it at a known filename in one of a few known places, so
 * those are tried in order. GitLab keeps any number of them in a
 * directory under names the project chose, so that one is listed rather
 * than guessed — asking for `Default.md` and giving up would miss most
 * real repositories.
 */
async function loadRepoTemplate(
  root: string,
  providerKind: string,
): Promise<string | null> {
  if (providerKind === "gitlab") {
    const dir = joinPath(root, GITLAB_TEMPLATE_DIR);
    const entries = await listDirectory(dir, false).catch(() => []);
    const first = entries.find((entry) => /\.(md|txt)$/i.test(entry.name));
    if (first) {
      const body = await readFile(joinPath(dir, first.name)).catch(() => null);
      if (body?.trim()) return body;
    }
  }
  for (const candidate of GITHUB_TEMPLATE_PATHS) {
    const body = await readFile(joinPath(root, candidate)).catch(() => null);
    if (body?.trim()) return body;
  }
  return null;
}

const LABEL =
  "font-mono text-[9.5px] font-semibold uppercase tracking-[0.07em] text-muted-foreground";

const FIELD =
  "w-full rounded-md border-0 bg-muted/60 px-2.5 py-1.5 text-[12px] text-foreground " +
  "outline-none transition-colors placeholder:text-muted-foreground/70 " +
  "focus-visible:ring-[1.5px] focus-visible:ring-ring/60";

/** A chip on the card background — the description's helpers. */
const CHIP =
  "inline-flex h-[22px] shrink-0 items-center gap-1 rounded-[5px] border-0 bg-card px-2 " +
  "text-[10px] text-foreground/90 transition-colors hover:bg-accent/50 " +
  "disabled:opacity-50 outline-none focus-visible:ring-[1.5px] focus-visible:ring-ring/60";

/** A chip that is an invitation rather than an action. */
const DASHED_CHIP =
  "inline-flex h-[22px] shrink-0 items-center gap-1 rounded-full border border-dashed " +
  "border-border px-2.5 text-[11px] text-muted-foreground transition-colors " +
  "hover:border-foreground/40 hover:text-foreground outline-none " +
  "focus-visible:ring-[1.5px] focus-visible:ring-ring/60";

export interface CreatePrFormProps {
  cwd: string;
  /** Where the repository's template lives — the worktree root is a
   *  checkout of the same repository, so either resolves it. */
  projectRoot: string;
  branchName: string | null;
  /** The branch the pull request targets unless the user picks another. */
  defaultBranch: string | null;
  provider: ProviderPresentation;
  onCreated: (pr: PullRequestInfo) => void;
  onCancel: () => void;
  /** Switch the pane deck to Changes — the warning row's next step. */
  onOpenChanges: () => void;
}

export function CreatePrForm({
  cwd,
  projectRoot,
  branchName,
  defaultBranch,
  provider,
  onCreated,
  onCancel,
  onOpenChanges,
}: CreatePrFormProps) {
  const [baseBranch, setBaseBranch] = useState(defaultBranch ?? "main");
  const [branches, setBranches] = useState<string[]>([]);
  const [commits, setCommits] = useState<CommitSummary[]>([]);

  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [titleSource, setTitleSource] = useState<DraftSource>("none");
  // Rule 04, in its smallest form: once the user has typed, nothing
  // re-drafts over them — not a base-branch change, not a re-poll — and
  // the "drafted from your commits" note comes off, because it is no
  // longer true.
  const [titleEdited, setTitleEdited] = useState(false);
  const [bodyEdited, setBodyEdited] = useState(false);

  const [template, setTemplate] = useState<string | null>(null);
  const [reviewers, setReviewers] = useState<string[]>([]);
  const [reviewerDraft, setReviewerDraft] = useState<string | null>(null);
  const [dirtyFiles, setDirtyFiles] = useState(0);
  const [hasUpstream, setHasUpstream] = useState<boolean | null>(null);

  const [busy, setBusy] = useState<null | "create" | "draft">(null);
  const [error, setError] = useState<string | null>(null);

  const reviewerInputRef = useRef<HTMLInputElement>(null);

  // ── What the branch already knows ──

  useEffect(() => {
    let cancelled = false;
    listBranches(cwd, false)
      .then((list) => !cancelled && setBranches(list))
      .catch(() => {});
    getGitStatus(cwd)
      .then((files) => !cancelled && setDirtyFiles(files.length))
      .catch(() => {});
    getGitBranchInfo(cwd)
      .then((info) => !cancelled && setHasUpstream(info.has_upstream))
      .catch(() => !cancelled && setHasUpstream(null));
    loadRepoTemplate(projectRoot, provider.kind)
      .then((found) => !cancelled && setTemplate(found))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [cwd, projectRoot, provider.kind]);

  // The commit set is bounded by the base branch, so changing the base
  // re-reads it — a pull request against `develop` is a different set of
  // commits than one against `main`, and so is a different description.
  useEffect(() => {
    let cancelled = false;
    gitCommitsAhead(cwd, baseBranch, MAX_COMMITS)
      .then((list) => !cancelled && setCommits(list))
      .catch(() => !cancelled && setCommits([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, baseBranch]);

  const drafted = useMemo(
    () => ({ title: draftTitle(commits, branchName), body: draftBody(commits) }),
    [commits, branchName],
  );

  useEffect(() => {
    if (!titleEdited) {
      setTitle(drafted.title.value);
      setTitleSource(drafted.title.source);
    }
    if (!bodyEdited) setBody(drafted.body.value);
  }, [drafted, titleEdited, bodyEdited]);

  // ── Creating ──

  const create = useCallback(
    async (draft: boolean) => {
      if (!title.trim() || busy) return;
      setBusy(draft ? "draft" : "create");
      setError(null);
      try {
        // The caption promises this, so it happens whether or not the
        // branch has ever been pushed — `--set-upstream` only when there
        // is no upstream to push to.
        await gitPushChanges(cwd, hasUpstream === false);

        const pr = await createPullRequest(
          cwd,
          title.trim(),
          body.trim(),
          baseBranch,
          draft,
        );

        // Reviewers are a second request the host only accepts once the
        // pull request exists. A failure here does not undo a pull
        // request that was created — it says which handle didn't take.
        const failed: string[] = [];
        for (const reviewer of reviewers) {
          await requestPrReview(cwd, pr.number, reviewer).catch(() =>
            failed.push(reviewer),
          );
        }
        if (failed.length > 0) {
          toast.warning(
            `${provider.nounTitle} created — couldn't request ${failed.join(", ")}`,
          );
        }

        onCreated(pr);
      } catch (err) {
        setError(String(err));
      } finally {
        setBusy(null);
      }
    },
    [title, body, baseBranch, busy, cwd, hasUpstream, reviewers, provider, onCreated],
  );

  const addReviewer = () => {
    const handle = (reviewerDraft ?? "").trim().replace(/^@/, "");
    if (handle && !reviewers.includes(handle)) setReviewers([...reviewers, handle]);
    setReviewerDraft(null);
  };

  const showDraftedNote = !titleEdited && titleSource === "commits";
  const commitCount = commits.length;

  return (
    <div className="flex flex-col" data-testid="create-pr-form">
      {/* ── Header: what is being opened, and against what ── */}
      <div className="flex flex-col gap-[7px] border-b border-border/40 px-3 pb-2.5 pt-2.5">
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            aria-label="Back"
            onClick={onCancel}
            className="-ml-1 flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
          >
            <ChevronLeft className="size-3.5" />
          </button>
          <span className="flex-1 text-[12.5px] font-semibold text-foreground">
            New {provider.noun}
          </span>
          {commitCount > 0 && (
            <span className="shrink-0 text-[10.5px] text-muted-foreground">
              {plural(commitCount, "commit")}
            </span>
          )}
        </div>
        <div className="flex min-w-0 items-center gap-1.5 font-mono text-[10.5px] text-muted-foreground">
          {/* Branch names truncate from the left: the end is the part
              that identifies them. */}
          <span
            dir="rtl"
            className="min-w-0 truncate text-left text-foreground/80"
            title={branchName ?? undefined}
          >
            {branchName ?? "this branch"}
          </span>
          <span className="shrink-0 opacity-60">→</span>
          <Select value={baseBranch} onValueChange={setBaseBranch}>
            <SelectTrigger
              aria-label="Base branch"
              data-testid="create-pr-base"
              className="h-auto shrink-0 gap-1 border-0 bg-transparent p-0 font-mono text-[10.5px] text-muted-foreground shadow-none data-[size=default]:h-auto hover:text-foreground dark:bg-transparent dark:hover:bg-transparent [&_svg]:size-3"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {(branches.includes(baseBranch) ? branches : [baseBranch, ...branches]).map(
                (branch) => (
                  <SelectItem key={branch} value={branch} className="font-mono text-xs">
                    {branch}
                  </SelectItem>
                ),
              )}
            </SelectContent>
          </Select>
        </div>
      </div>

      {/* ── Body ── */}
      <div className="flex flex-col gap-3 px-3 py-3">
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-1.5">
            <label className={`${LABEL} flex-1`} htmlFor="create-pr-title">
              Title
            </label>
            {showDraftedNote && (
              <span
                data-testid="create-pr-drafted-note"
                className="flex items-center gap-1.5 text-[10px] text-accent-ember"
              >
                <span className="size-[5px] rounded-full bg-accent-ember" />
                drafted from your commits
              </span>
            )}
          </div>
          <input
            id="create-pr-title"
            className={FIELD}
            value={title}
            placeholder={`${provider.nounTitle} title`}
            onChange={(event) => {
              setTitleEdited(true);
              setTitle(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") void create(false);
            }}
          />
        </div>

        <div className="flex flex-col gap-1.5">
          <label className={LABEL} htmlFor="create-pr-body">
            Description
          </label>
          <textarea
            id="create-pr-body"
            className={`${FIELD} min-h-[118px] resize-y leading-relaxed`}
            value={body}
            placeholder="What this changes, and how you checked."
            onChange={(event) => {
              setBodyEdited(true);
              setBody(event.target.value);
            }}
          />
          {template && (
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                className={CHIP}
                data-testid="create-pr-template"
                onClick={() => {
                  setBodyEdited(true);
                  setBody((current) =>
                    current.trim() ? `${template.trim()}\n\n${current}` : template,
                  );
                }}
              >
                Use repo template
              </button>
            </div>
          )}
        </div>

        {/* Reviewers are collected here and requested after the pull
            request exists — the host has nothing to attach them to
            until then. */}
        <div className="flex flex-wrap items-center gap-1.5">
          {reviewers.map((reviewer) => (
            <span
              key={reviewer}
              className="inline-flex h-[22px] items-center gap-1 rounded-full bg-card px-2.5 text-[11px] text-foreground/90"
            >
              {reviewer}
              <button
                type="button"
                aria-label={`Remove ${reviewer}`}
                className="text-muted-foreground transition-colors hover:text-foreground"
                onClick={() => setReviewers(reviewers.filter((r) => r !== reviewer))}
              >
                <X className="size-2.5" />
              </button>
            </span>
          ))}
          {reviewerDraft == null ? (
            <button
              type="button"
              className={DASHED_CHIP}
              data-testid="create-pr-add-reviewer"
              onClick={() => {
                setReviewerDraft("");
                requestAnimationFrame(() => reviewerInputRef.current?.focus());
              }}
            >
              + reviewer
            </button>
          ) : (
            <input
              ref={reviewerInputRef}
              aria-label="Reviewer handle"
              className="h-[22px] w-28 rounded-full border-0 bg-muted/60 px-2.5 text-[11px] outline-none focus-visible:ring-[1.5px] focus-visible:ring-ring/60"
              value={reviewerDraft}
              placeholder="handle"
              onChange={(event) => setReviewerDraft(event.target.value)}
              onBlur={addReviewer}
              onKeyDown={(event) => {
                if (event.key === "Enter") addReviewer();
                if (event.key === "Escape") setReviewerDraft(null);
              }}
            />
          )}
        </div>

        {dirtyFiles > 0 && (
          <div
            data-testid="create-pr-dirty"
            className="flex items-center gap-2 rounded-md bg-status-working/[0.08] px-2.5 py-2"
          >
            <span className="size-2 shrink-0 rounded-full bg-status-working" />
            <span className="flex-1 text-[11px] text-foreground/85">
              {plural(dirtyFiles, "file")} {dirtyFiles === 1 ? "has" : "have"} uncommitted
              changes
            </span>
            <button type="button" className={CHIP} onClick={onOpenChanges}>
              Review
            </button>
          </div>
        )}

        {error && (
          <div
            data-testid="create-pr-error"
            className="flex items-start gap-2 rounded-md bg-status-attention/[0.08] px-2.5 py-2"
          >
            <span className="mt-1 size-2 shrink-0 rounded-full bg-status-attention" />
            <span className="flex-1 break-words text-[11px] text-foreground/85">
              {error} — your title and description are still here.
            </span>
          </div>
        )}
      </div>

      {/* ── Footer: the promise, and the two ways to keep it ── */}
      <div className="flex items-center gap-1.5 border-t border-border/40 bg-muted/40 px-3 py-2.5">
        <span className="flex-1 text-[10.5px] text-muted-foreground">
          Pushes the branch first
        </span>
        {/* Both buttons keep their box in flight (binding rule 1): the
            label changes and a spinner appears inside the same slot. */}
        <button
          type="button"
          className={`${btnCard} min-w-[58px]`}
          disabled={!title.trim() || busy != null}
          onClick={() => void create(true)}
        >
          {busy === "draft" ? <Loader2 className="size-3 animate-spin" /> : null}
          Draft
        </button>
        <button
          type="button"
          className={`${btnEmberSolid} min-w-[68px]`}
          data-testid="create-pr-submit"
          disabled={!title.trim() || busy != null}
          onClick={() => void create(false)}
        >
          {busy === "create" ? <Loader2 className="size-3 animate-spin" /> : null}
          {busy === "create" ? "Creating" : "Create"}
        </button>
      </div>
    </div>
  );
}
