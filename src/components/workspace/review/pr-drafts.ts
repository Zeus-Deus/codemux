/**
 * In-memory scratch state for the review surfaces.
 *
 * Binding rule 4: anything typed survives everything. A 2.5s poll, a
 * state flip, a tab switch, a failed submit and a remount must all leave
 * a half-written review or an edited description exactly where it was.
 * React state alone can't promise that — the panel unmounts when the
 * pane deck switches tabs — so the text lives here, outside the tree.
 *
 * Equally deliberate: it is *only* here. Nothing is written to disk or
 * to the backend. A two-day-old draft resurfacing on a PR you've since
 * forgotten is worse than retyping a sentence, so these die with the
 * page.
 */

/** `${workspaceId}:${prNumber}` — a draft belongs to one PR in one
 *  workspace, not to a branch name that may get reused. */
export type DraftKey = string;

export function draftKey(workspaceId: string, prNumber: number): DraftKey {
  return `${workspaceId}:${prNumber}`;
}

const reviewDrafts = new Map<DraftKey, string>();
const descriptionDrafts = new Map<DraftKey, string>();
const descriptionFolds = new Map<DraftKey, boolean>();
const lastVerdicts = new Map<DraftKey, string>();

/** Merge strategy is a habit, not a per-PR decision, so it is
 *  remembered once for the session. */
let mergeStrategy = "squash";

export function getReviewDraft(key: DraftKey): string {
  return reviewDrafts.get(key) ?? "";
}

export function setReviewDraft(key: DraftKey, value: string): void {
  if (value) reviewDrafts.set(key, value);
  else reviewDrafts.delete(key);
}

export function clearReviewDraft(key: DraftKey): void {
  reviewDrafts.delete(key);
}

/** `undefined` means "not being edited"; `""` is a deliberately emptied
 *  description and must not collapse back to the server's copy. */
export function getDescriptionDraft(key: DraftKey): string | undefined {
  return descriptionDrafts.get(key);
}

export function setDescriptionDraft(key: DraftKey, value: string): void {
  descriptionDrafts.set(key, value);
}

export function clearDescriptionDraft(key: DraftKey): void {
  descriptionDrafts.delete(key);
}

/** Fold state is remembered per PR — a long description you folded once
 *  stays folded while you work through the checks. */
export function getDescriptionFold(key: DraftKey): boolean | undefined {
  return descriptionFolds.get(key);
}

export function setDescriptionFold(key: DraftKey, folded: boolean): void {
  descriptionFolds.set(key, folded);
}

export function getLastVerdict(key: DraftKey): string {
  return lastVerdicts.get(key) ?? "comment";
}

export function setLastVerdict(key: DraftKey, verdict: string): void {
  lastVerdicts.set(key, verdict);
}

export function getMergeStrategy(): string {
  return mergeStrategy;
}

export function setMergeStrategy(strategy: string): void {
  mergeStrategy = strategy;
}

/** Reset every scratch map — for tests only. */
export function _resetPrDrafts(): void {
  reviewDrafts.clear();
  descriptionDrafts.clear();
  descriptionFolds.clear();
  lastVerdicts.clear();
  mergeStrategy = "squash";
}
