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

import { useSyncExternalStore } from "react";
import {
  indexDiffRows,
  reanchor,
  type AnchorSide,
  type DraftAnchor,
} from "@/lib/pr-anchor";

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

// ── Line notes ──────────────────────────────────────────────────────
//
// The pending review: notes written against lines of the diff, held
// here for exactly as long as the tab lives. Same rule as the text
// above — a tab switch, a PR switch, a refetch, a failed submit and a
// remount all leave them alone; closing the app does not.

/** Whether we still know which line this note belongs to. */
export type DraftAnchorStatus = "pinned" | "moved" | "unanchored";

export interface LineDraft {
  id: string;
  path: string;
  side: AnchorSide;
  /** End of the selection — and the whole anchor for a one-line note. */
  line: number;
  /** Start of a multi-line selection; null when it is a single line. */
  startLine: number | null;
  lineText: string;
  startLineText: string | null;
  contextBefore: string | null;
  contextAfter: string | null;
  hunkHeader: string;
  /** The head the diff was showing when this was written. Submitting
   *  against a different one is the silent-mispin trap. */
  headOidAtDraft: string;
  body: string;
  status: DraftAnchorStatus;
  /** Where it sat before the matcher moved it — for "moved 47 → 50". */
  movedFrom: number | null;
}

export type NewLineDraft = Omit<LineDraft, "id" | "status" | "movedFrom">;

const lineDrafts = new Map<DraftKey, LineDraft[]>();
/** Per PR: the raw diff text as it was at a given head oid, so "show me
 *  what I was looking at" doesn't depend on a force-pushed commit still
 *  being fetchable. It usually isn't. */
const diffSnapshots = new Map<DraftKey, Map<string, string>>();
const viewedFiles = new Map<DraftKey, Set<string>>();

const EMPTY: LineDraft[] = [];
const EMPTY_SET: ReadonlySet<string> = new Set();

let seq = 0;
const listeners = new Set<() => void>();

function notify(): void {
  for (const fn of listeners) fn();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Replace a key's list wholesale — the array identity is the snapshot
 *  `useSyncExternalStore` compares, so mutation in place is not an
 *  option. */
function put(key: DraftKey, next: LineDraft[]): void {
  if (next.length) lineDrafts.set(key, next);
  else lineDrafts.delete(key);
  notify();
}

export function getLineDrafts(key: DraftKey): LineDraft[] {
  return lineDrafts.get(key) ?? EMPTY;
}

/** Subscribe a component to every PR's notes; the footer and the Code
 *  tab both need to repaint when either of them changes one. */
export function useLineDrafts(key: DraftKey): LineDraft[] {
  return useSyncExternalStore(subscribe, () => getLineDrafts(key), () => EMPTY);
}

export function addLineDraft(key: DraftKey, draft: NewLineDraft): LineDraft {
  const created: LineDraft = {
    ...draft,
    id: `note-${++seq}`,
    status: "pinned",
    movedFrom: null,
  };
  put(key, [...getLineDrafts(key), created]);
  return created;
}

export function updateLineDraft(
  key: DraftKey,
  id: string,
  patch: Partial<LineDraft>,
): void {
  put(
    key,
    getLineDrafts(key).map((d) => (d.id === id ? { ...d, ...patch } : d)),
  );
}

export function removeLineDraft(key: DraftKey, id: string): void {
  put(
    key,
    getLineDrafts(key).filter((d) => d.id !== id),
  );
}

export function clearLineDrafts(key: DraftKey): void {
  put(key, []);
}

/** "3 pending on 2 files" — the footer's whole sentence. */
export function draftCounts(drafts: LineDraft[]): {
  notes: number;
  files: number;
  unanchored: number;
} {
  return {
    notes: drafts.length,
    files: new Set(drafts.map((d) => d.path)).size,
    unanchored: drafts.filter((d) => d.status === "unanchored").length,
  };
}

export function putDiffSnapshot(key: DraftKey, headOid: string, diff: string): void {
  let snapshots = diffSnapshots.get(key);
  if (!snapshots) {
    snapshots = new Map();
    diffSnapshots.set(key, snapshots);
  }
  if (snapshots.has(headOid)) return;
  // Three is enough to answer "what did it look like before?" across a
  // couple of force-pushes without holding every diff of a busy
  // afternoon in memory.
  if (snapshots.size >= 3) {
    const oldest = snapshots.keys().next().value;
    if (oldest) snapshots.delete(oldest);
  }
  snapshots.set(headOid, diff);
}

export function getDiffSnapshot(key: DraftKey, headOid: string): string | null {
  return diffSnapshots.get(key)?.get(headOid) ?? null;
}

/** Files you've marked read, per PR. A collapsed file is a file you
 *  have decided about, which is why this is not persisted either. */
export function isFileViewed(key: DraftKey, path: string): boolean {
  return viewedFiles.get(key)?.has(path) ?? false;
}

export function toggleFileViewed(key: DraftKey, path: string): void {
  const set = new Set(viewedFiles.get(key) ?? EMPTY_SET);
  if (set.has(path)) set.delete(path);
  else set.add(path);
  viewedFiles.set(key, set);
  notify();
}

/** The viewed set as a value: a new Set on every toggle, so a component
 *  reading it through `useSyncExternalStore` actually repaints. */
export function useViewedFiles(key: DraftKey): ReadonlySet<string> {
  return useSyncExternalStore(
    subscribe,
    () => viewedFiles.get(key) ?? EMPTY_SET,
    () => EMPTY_SET,
  );
}

// ── Diff view preferences ──
//
// Split-vs-unified and whitespace are habits, not per-PR decisions —
// remembered once for the session, like the merge strategy above.

export type DiffLayout = "unified" | "split";

let diffLayout: DiffLayout = "unified";
let ignoreWhitespacePref = false;

export function getDiffLayout(): DiffLayout {
  return diffLayout;
}

export function setDiffLayout(layout: DiffLayout): void {
  diffLayout = layout;
}

export function getIgnoreWhitespace(): boolean {
  return ignoreWhitespacePref;
}

export function setIgnoreWhitespace(value: boolean): void {
  ignoreWhitespacePref = value;
}

// ── Re-anchoring ──

export interface ReanchorSummary {
  moved: number;
  unanchored: number;
  unchanged: number;
}

function anchorOf(draft: LineDraft, which: "start" | "end"): DraftAnchor {
  return which === "end"
    ? {
        path: draft.path,
        side: draft.side,
        line: draft.line,
        text: draft.lineText,
        contextBefore: draft.startLine == null ? draft.contextBefore : null,
        contextAfter: draft.contextAfter,
      }
    : {
        path: draft.path,
        side: draft.side,
        line: draft.startLine!,
        text: draft.startLineText!,
        contextBefore: draft.contextBefore,
        contextAfter: null,
      };
}

/**
 * Point every note at the diff that is on screen now.
 *
 * Called whenever the head oid a note was written against stops being
 * the current one. A note is moved only when the matcher is certain;
 * otherwise it is kept, marked unanchored and said out loud. Nothing is
 * deleted here — the words you wrote are the part that took effort.
 */
export function reanchorLineDrafts(
  key: DraftKey,
  diffText: string,
  headOid: string,
): ReanchorSummary {
  const drafts = getLineDrafts(key);
  if (!drafts.length) return { moved: 0, unanchored: 0, unchanged: 0 };

  const index = indexDiffRows(diffText);
  const summary: ReanchorSummary = { moved: 0, unanchored: 0, unchanged: 0 };

  const next = drafts.map((draft): LineDraft => {
    if (draft.headOidAtDraft === headOid && draft.status !== "unanchored") {
      summary.unchanged++;
      return draft;
    }

    const end = reanchor(anchorOf(draft, "end"), index);
    const start =
      draft.startLine != null && draft.startLineText != null
        ? reanchor(anchorOf(draft, "start"), index)
        : null;

    const lostRange = start != null && start.status === "lost";
    if (end.status === "lost" || lostRange) {
      summary.unanchored++;
      return { ...draft, status: "unanchored" };
    }

    const newStart = start?.status === "reanchored" ? start.line : null;
    // A range whose ends crossed or collapsed is not the range you
    // selected, whatever the two matches say individually.
    if (draft.startLine != null && (newStart == null || newStart >= end.line)) {
      summary.unanchored++;
      return { ...draft, status: "unanchored" };
    }

    const moved = end.moved || (newStart != null && newStart !== draft.startLine);
    if (moved) summary.moved++;
    else summary.unchanged++;

    return {
      ...draft,
      line: end.line,
      startLine: newStart,
      hunkHeader: end.hunk,
      headOidAtDraft: headOid,
      status: moved ? "moved" : "pinned",
      movedFrom: moved ? (draft.movedFrom ?? draft.line) : null,
    };
  });

  put(key, next);
  return summary;
}

/**
 * Why this pending review can't be sent yet, in words — or null.
 *
 * Submitting is all-or-nothing at the host: one anchor GitHub can't
 * resolve rejects the entire review with a 422 and you get nothing
 * back. So the check happens here, before anything leaves.
 */
export function submitBlockedReason(
  drafts: LineDraft[],
  headOid: string | null,
): string | null {
  const unanchored = drafts.filter((d) => d.status === "unanchored");
  if (unanchored.length) {
    const files = new Set(unanchored.map((d) => d.path)).size;
    return `${unanchored.length === 1 ? "1 note" : `${unanchored.length} notes`} on ${
      files === 1 ? "1 file" : `${files} files`
    } no longer match a line. The host rejects a review if any one of its comments doesn't resolve, so all of them would fail.`;
  }
  if (headOid) {
    const stale = drafts.filter((d) => d.headOidAtDraft !== headOid);
    if (stale.length) {
      return `${
        stale.length === 1 ? "1 note was" : `${stale.length} notes were`
      } written against an older version of this branch. Re-anchor them first — sending them as they are would pin them to a commit nobody is looking at.`;
    }
  }
  return null;
}

/** Reset every scratch map — for tests only. */
export function _resetPrDrafts(): void {
  reviewDrafts.clear();
  descriptionDrafts.clear();
  descriptionFolds.clear();
  lastVerdicts.clear();
  lineDrafts.clear();
  diffSnapshots.clear();
  viewedFiles.clear();
  mergeStrategy = "squash";
  diffLayout = "unified";
  ignoreWhitespacePref = false;
  seq = 0;
  notify();
}
