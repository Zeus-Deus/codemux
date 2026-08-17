/**
 * Drafting a pull request out of the commits that are already on the
 * branch.
 *
 * The premise of the create form: an agent (or you) just wrote these
 * commits in this workspace, so a title and a description already exist
 * in prose — they are only spread across N commit messages. The form's
 * job is to assemble them, not to ask for them again.
 *
 * Two rules run through everything here:
 *
 * - **The note is a claim, not decoration.** "drafted from your commits"
 *   is rendered only when commit text actually produced the field. When
 *   the commits had nothing usable and the branch name is all there is,
 *   the field is still filled in — but nothing claims the commits wrote
 *   it.
 * - **Nothing is invented.** No Verification line unless a commit body
 *   carried one. A description that would have to be guessed at is left
 *   for the user to write.
 *
 * Pure functions over plain data, so every heuristic here is testable
 * without mounting the form.
 */

import type { CommitSummary } from "@/tauri/types";

/** Where a drafted field's words came from. Only `commits` earns the note. */
export type DraftSource = "commits" | "branch" | "none";

export interface DraftedField {
  value: string;
  source: DraftSource;
}

// ── Branch names ──────────────────────────────────────────────────────

/**
 * `feat/drop-ports-agent-guidance` → `Drop ports agent guidance`.
 *
 * Leading type segments are dropped rather than title-cased: a title
 * reading "Feat drop ports…" is worse than one that never mentions the
 * prefix, and the conventional-commit type is re-attached separately
 * when the commits agree on one.
 */
export function humanizeBranch(branch: string | null): string {
  if (!branch) return "";
  const withoutOwner = branch.replace(/^[^/]+\/(?=.*\/)/, "");
  const stripped = withoutOwner.replace(
    /^(feat|feature|fix|hotfix|chore|docs|refactor|test|perf|build|ci|style)[/_-]/i,
    "",
  );
  const words = stripped
    .replace(/[/_-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!words) return "";
  return words.charAt(0).toUpperCase() + words.slice(1);
}

// ── Conventional commits ──────────────────────────────────────────────

interface Conventional {
  /** `feat`, `fix`, … lowercased. */
  type: string;
  /** Scope inside the parentheses, or null when there wasn't one. */
  scope: string | null;
  /** Everything after the colon. */
  rest: string;
}

const CONVENTIONAL = /^([a-z]+)(?:\(([^)]+)\))?!?:\s*(.+)$/i;

export function parseConventional(subject: string): Conventional | null {
  const match = CONVENTIONAL.exec(subject.trim());
  if (!match) return null;
  return {
    type: match[1].toLowerCase(),
    scope: match[2]?.trim() || null,
    rest: match[3].trim(),
  };
}

/**
 * The `type(scope)` every one of these subjects agrees on, if any.
 *
 * All-or-nothing on purpose. A branch where three commits say `feat` and
 * one says `fix` has no single classification, and picking the majority
 * would put a word in the title that a quarter of the work contradicts.
 */
function commonPrefix(parsed: Conventional[]): string | null {
  if (parsed.length === 0) return null;
  const type = parsed[0].type;
  if (!parsed.every((c) => c.type === type)) return null;
  const scope = parsed[0].scope;
  const sameScope = parsed.every((c) => c.scope === scope);
  return sameScope && scope ? `${type}(${scope})` : type;
}

/** The words every remainder starts with — `[]` when they diverge at once. */
function commonWordPrefix(texts: string[]): string[] {
  if (texts.length === 0) return [];
  const split = texts.map((t) => t.split(/\s+/).filter(Boolean));
  const shared: string[] = [];
  for (let i = 0; i < split[0].length; i++) {
    const word = split[0][i];
    if (!split.every((words) => words[i]?.toLowerCase() === word.toLowerCase())) break;
    shared.push(word);
  }
  return shared;
}

// ── Title ─────────────────────────────────────────────────────────────

/**
 * A title for these commits.
 *
 * - One commit: its subject, verbatim. It already is the title.
 * - Several with a shared `type(scope)`: that classification, plus
 *   whatever leading words the subjects share — and when they share
 *   fewer than two, the branch name as the summary. The prefix came
 *   from the commits either way, so the note stands.
 * - Several that agree on nothing: the branch name, with no note. Three
 *   unrelated subjects cannot be summarised without inventing a summary.
 */
export function draftTitle(
  commits: CommitSummary[],
  branch: string | null,
): DraftedField {
  const branchTitle = humanizeBranch(branch);

  if (commits.length === 0) {
    return { value: branchTitle, source: branchTitle ? "branch" : "none" };
  }
  if (commits.length === 1) {
    const subject = commits[0].subject.trim();
    if (subject) return { value: subject, source: "commits" };
    return { value: branchTitle, source: branchTitle ? "branch" : "none" };
  }

  // Oldest first: a shared prefix reads in the order the work happened.
  const subjects = [...commits].reverse().map((c) => c.subject.trim()).filter(Boolean);
  const parsed = subjects.map(parseConventional);
  if (parsed.every((p): p is Conventional => p !== null)) {
    const prefix = commonPrefix(parsed);
    if (prefix) {
      const shared = commonWordPrefix(parsed.map((p) => p.rest));
      const summary =
        shared.length >= 2 ? shared.join(" ") : branchTitle.toLowerCase();
      if (summary) return { value: `${prefix}: ${summary}`, source: "commits" };
    }
  }

  return { value: branchTitle, source: branchTitle ? "branch" : "none" };
}

// ── Description ───────────────────────────────────────────────────────

/** Trailers are metadata for the commit, not prose for the description. */
const TRAILER =
  /^(co-authored-by|signed-off-by|reviewed-by|refs|closes|fixes|resolves|change-id|see-also)\s*:/i;

/** Verification evidence a commit body already stated, in its own words. */
const VERIFICATION_LINE = /^(verification|verified|tested|test plan|testing)\s*[:·-]\s*(.+)$/i;

function withoutTrailers(body: string): string {
  return body
    .split("\n")
    .filter((line) => !TRAILER.test(line.trim()))
    .join("\n")
    .trim();
}

/**
 * A verification line, only if one of the commits already made the claim.
 *
 * Never assembled from what the tooling could have run. A description
 * that says the tests pass when nobody checked is worse than one that
 * says nothing — the reviewer would have believed it.
 */
export function verificationFrom(commits: CommitSummary[]): string | null {
  for (const commit of commits) {
    for (const line of commit.body.split("\n")) {
      const match = VERIFICATION_LINE.exec(line.trim());
      if (match && match[2].trim()) return match[2].trim();
    }
  }
  return null;
}

/**
 * A description for these commits.
 *
 * - One commit with a body: that body, minus its trailers. The author
 *   already wrote the description; it is sitting in the commit.
 * - One commit with no body: nothing. Repeating the title as the
 *   description is noise.
 * - Several: their subjects as a list, oldest first — the shape of the
 *   branch, which is what a reviewer opens the description for.
 *
 * A `Verification · …` line is appended only when a commit body stated
 * one (see {@link verificationFrom}).
 */
export function draftBody(commits: CommitSummary[]): DraftedField {
  if (commits.length === 0) return { value: "", source: "none" };

  const verification = verificationFrom(commits);
  const suffix = verification ? `\n\nVerification · ${verification}` : "";

  if (commits.length === 1) {
    const body = withoutTrailers(commits[0].body);
    // The verification line is already inside the body it came from.
    if (body) return { value: body, source: "commits" };
    return verification
      ? { value: suffix.trim(), source: "commits" }
      : { value: "", source: "none" };
  }

  const bullets = [...commits]
    .reverse()
    .map((c) => c.subject.trim())
    .filter(Boolean)
    .map((subject) => `- ${subject}`);
  if (bullets.length === 0) return { value: "", source: "none" };
  return { value: `${bullets.join("\n")}${suffix}`, source: "commits" };
}

// ── Repository templates ──────────────────────────────────────────────

/**
 * Where each host keeps its pull-request template.
 *
 * GitHub's is a known filename in one of a few known places. GitLab's is
 * any `.md` inside a directory, so that one is listed rather than
 * guessed — see `loadRepoTemplate` in the form.
 */
export const GITHUB_TEMPLATE_PATHS = [
  ".github/PULL_REQUEST_TEMPLATE.md",
  ".github/pull_request_template.md",
  ".github/PULL_REQUEST_TEMPLATE/default.md",
  "PULL_REQUEST_TEMPLATE.md",
  "pull_request_template.md",
  "docs/PULL_REQUEST_TEMPLATE.md",
] as const;

export const GITLAB_TEMPLATE_DIR = ".gitlab/merge_request_templates";

/** `a/b` joined to a root without caring which slash the platform uses. */
export function joinPath(root: string, relative: string): string {
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  const trimmed = root.replace(/[/\\]+$/, "");
  return `${trimmed}${sep}${relative.split("/").join(sep)}`;
}
