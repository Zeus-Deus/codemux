import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { addPrInlineComment } from "@/tauri/commands";
import { anchorContext, indexDiffRows } from "@/lib/pr-anchor";
import { splitDiffFiles, type PrDiffFile } from "@/lib/pr-diff";
import type { DiffLine } from "@/lib/diff-parser";
import type { DiffRowSide, DiffSelection } from "@/components/diff/diff-row";
import type { ReviewThreadTask } from "@/lib/pr-agent-handoff";
import type { PrReviewThread } from "@/tauri/types";
import { ReviewCodeFile } from "./review-code-file";
import { ReviewLineComposer } from "./review-line-composer";
import {
  CodeThread,
  threadSide,
  useOptimisticResolve,
  type SuggestionResolver,
} from "./review-threads";
import {
  btnCard,
  tzBody,
  tzBodyLg,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
  underRowInset,
} from "./review-ui";
import {
  addLineDraft,
  getDiffSnapshot,
  getDiffLayout,
  getIgnoreWhitespace,
  removeLineDraft,
  setDiffLayout,
  setIgnoreWhitespace,
  toggleFileViewed,
  updateLineDraft,
  useLineDrafts,
  useViewedFiles,
  type DiffLayout,
  type DraftKey,
  type LineDraft,
} from "./pr-drafts";

/**
 * A nudge from the detail surface, which owns the drift notice: bump
 * the nonce to enter a mode. A nonce rather than a boolean so the same
 * action can be taken twice in a row.
 */
export type CodeTabIntent =
  | { kind: "repin" | "old-diff"; nonce: number }
  /** Scroll to one line and mark it — a thread's anchor was clicked. */
  | { kind: "focus"; nonce: number; path: string; side: DiffRowSide; line: number };

interface Props {
  draftKey: DraftKey;
  cwd: string;
  prNumber: number;
  /** Head the rendered diff belongs to; every note records it. */
  headOid: string | null;
  diffText: string;
  loading: boolean;
  error: string | null;
  /**
   * Whether line notes can be written against this host at all.
   *
   * False leaves the diff readable and inert: no gutter selection, no
   * composer, no pending notes. Drafting a note that can never be
   * submitted is worse than not offering one — the work is lost at the
   * last step, which is the step that matters.
   */
  canDraftLineNotes: boolean;
  /** "Comment now" additionally needs a host that takes one comment
   *  outside a review. */
  canCommentNow: boolean;
  onPosted: () => void;
  intent: CodeTabIntent | null;
  /**
   * Called with an intent's nonce once it has been acted on, so the
   * owner can drop it. This tab unmounts whenever another tab is shown,
   * and a pending intent would otherwise replay on every return here.
   */
  onIntentHandled?: (nonce: number) => void;
  /**
   * The conversation already on this diff, drawn under the lines it is
   * about so nobody repeats a point a reviewer or a bot already made.
   */
  threads?: PrReviewThread[];
  onSendToAgent?: (task: ReviewThreadTask) => Promise<unknown>;
  onReply?: (thread: PrReviewThread, body: string) => Promise<unknown>;
  onSetResolved?: (thread: PrReviewThread, resolved: boolean) => Promise<unknown>;
  suggestionTargetFor?: SuggestionResolver;
}

const NO_THREADS: PrReviewThread[] = [];

function threadKey(path: string, side: DiffRowSide, line: number): string {
  return `${path}\u0000${side}\u0000${line}`;
}

/**
 * A brief ember wash on a row someone was sent to, so the eye lands on
 * it after the scroll. Skipped under reduced motion, where the scroll
 * alone is the signal.
 *
 * Painted as a full-size inset shadow, which sits over the row's own
 * background instead of replacing it: animating `backgroundColor` wiped
 * the green or red diff tint for the length of the flash. The wash holds
 * for the first part so it is still there when a smooth scroll lands.
 */
const FLASH_WASH = "inset 0 0 0 100vmax color-mix(in oklch, var(--accent-ember) 25%, transparent)";
const FLASH_CLEAR = "inset 0 0 0 100vmax transparent";

function flashRow(el: HTMLElement) {
  if (typeof el.animate !== "function") return;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  el.animate(
    [
      { boxShadow: FLASH_WASH, offset: 0 },
      { boxShadow: FLASH_WASH, offset: 0.4 },
      { boxShadow: FLASH_CLEAR, offset: 1 },
    ],
    { duration: 900, easing: "ease-out" },
  );
}

/**
 * The row a focus lands on. Unified layout draws an unchanged line once,
 * addressed by its new number, so a thread written on the old side of it
 * has no `LEFT:<old>` row; it is found through the parsed file instead.
 */
function findFocusRow(
  fileEl: HTMLElement,
  lines: DiffLine[] | undefined,
  side: DiffRowSide,
  line: number,
): HTMLElement | null {
  const direct = fileEl.querySelector<HTMLElement>(`[data-diff-row="${side}:${line}"]`);
  if (direct || side !== "LEFT" || !lines) return direct;
  const context = lines.find(
    (l) => l.type === "context" && l.oldLine === line && l.newLine != null,
  );
  return context
    ? fileEl.querySelector<HTMLElement>(`[data-diff-row="RIGHT:${context.newLine}"]`)
    : null;
}

function scrollBehavior(): ScrollBehavior {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
}

interface Selection {
  path: string;
  side: DiffRowSide;
  /** Where the click landed; shift-click extends away from it. */
  anchor: number;
  start: number;
  end: number;
}

export function ReviewCodeTab({
  draftKey,
  cwd,
  prNumber,
  headOid,
  diffText,
  loading,
  error,
  canDraftLineNotes,
  canCommentNow,
  onPosted,
  intent,
  onIntentHandled,
  threads = NO_THREADS,
  onSendToAgent,
  onReply,
  onSetResolved,
  suggestionTargetFor,
}: Props) {
  const drafts = useLineDrafts(draftKey);
  const viewed = useViewedFiles(draftKey);
  const [layout, setLayoutState] = useState<DiffLayout>(() => getDiffLayout());
  const [hideWhitespace, setHideWhitespaceState] = useState(() => getIgnoreWhitespace());
  const [selection, setSelection] = useState<Selection | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [posting, setPosting] = useState(false);
  /** Non-null while the user is picking a new line for a lost note. */
  const [repinId, setRepinId] = useState<string | null>(null);
  /** Non-null while showing the diff a note was written against. */
  const [oldDiffOid, setOldDiffOid] = useState<string | null>(null);
  /** The line a thread's anchor sent us to; its file stays open. */
  const [focus, setFocus] = useState<{
    path: string;
    side: DiffRowSide;
    line: number;
    nonce: number;
  } | null>(null);
  const { resolvedOf, toggleResolved } = useOptimisticResolve(threads, onSetResolved);
  const rootRef = useRef<HTMLDivElement>(null);

  /**
   * What has been typed into the new-note composer, kept out here.
   *
   * The composer mounts beneath the *end* of the selection, so
   * shift-clicking to extend a range relocates it — and React remounts a
   * relocated component, which used to take everything typed into it
   * along. This surface's whole rule is that anything typed survives
   * everything, and widening a range while writing about it is the most
   * ordinary way there is to lose a paragraph.
   *
   * A ref, not state: this component is the parent of every file's diff
   * view, and re-rendering that tree on each keystroke would repaint
   * thousands of rows to update one textarea. The composer keeps its own
   * local state and reports changes here; this copy exists only to seed
   * the next mount. It is emptied whenever the note being written
   * changes — a different anchor, a submit, a cancel.
   */
  const composerDraft = useRef("");

  const unanchored = useMemo(
    () => drafts.filter((d) => d.status === "unanchored"),
    [drafts],
  );
  const repinPath = repinId ? (drafts.find((d) => d.id === repinId)?.path ?? null) : null;

  // ── Intents from the drift notice ──
  // The ref only guards against handling one intent twice in this mount
  // (the effect reruns as drafts change); the owner clearing it is what
  // stops a remount from treating it as new.
  const handledNonce = useRef(0);
  useEffect(() => {
    if (!intent || intent.nonce === handledNonce.current) return;
    handledNonce.current = intent.nonce;
    onIntentHandled?.(intent.nonce);
    if (intent.kind === "focus") {
      setOldDiffOid(null);
      setFocus({
        path: intent.path,
        side: intent.side,
        line: intent.line,
        nonce: intent.nonce,
      });
      return;
    }
    if (intent.kind === "old-diff") {
      const oid = drafts.find((d) => d.status === "unanchored")?.headOidAtDraft ?? null;
      setOldDiffOid(oid && getDiffSnapshot(draftKey, oid) ? oid : null);
      if (oid && !getDiffSnapshot(draftKey, oid)) {
        toast.info("No snapshot of that diff — it was never rendered here.");
      }
      return;
    }
    const first = drafts.find((d) => d.status === "unanchored");
    if (first) {
      setOldDiffOid(null);
      setRepinId(first.id);
      scrollToFile(first.path);
    }
  }, [intent, drafts, draftKey, onIntentHandled]);

  const scrollToFile = (path: string) => {
    // Deferred a frame: the file may only expand as a result of the
    // state change that asked for the scroll.
    requestAnimationFrame(() => {
      rootRef.current
        ?.querySelector(`[data-file-path="${CSS.escape(path)}"]`)
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
  };

  const showingOld = oldDiffOid != null;
  const renderedDiff = showingOld
    ? (getDiffSnapshot(draftKey, oldDiffOid) ?? diffText)
    : diffText;

  const files = useMemo(() => splitDiffFiles(renderedDiff), [renderedDiff]);
  const anchorIndex = useMemo(() => indexDiffRows(diffText), [diffText]);

  // Scroll once the target can exist: the diff may still be loading when
  // the tab opens, and the file only expands on the render after `focus`.
  const scrolledNonce = useRef(0);
  useEffect(() => {
    if (!focus || focus.nonce === scrolledNonce.current || files.length === 0) return;
    scrolledNonce.current = focus.nonce;
    requestAnimationFrame(() => {
      const file = rootRef.current?.querySelector<HTMLElement>(
        `[data-file-path="${CSS.escape(focus.path)}"]`,
      );
      const lines = files.find((f) => f.path === focus.path)?.lines;
      const row = file ? findFocusRow(file, lines, focus.side, focus.line) : null;
      // A file too large to render by default has no rows yet; its
      // header is the nearest honest place to land.
      (row ?? file)?.scrollIntoView({ behavior: scrollBehavior(), block: "center" });
      if (row) flashRow(row);
    });
  }, [focus, files]);

  /** Threads by the coordinate they hang under. Outdated threads have no
   *  line in this diff and stay on Summary. */
  const threadsByRow = useMemo(() => {
    const map = new Map<string, PrReviewThread[]>();
    for (const thread of threads) {
      if (!thread.path || thread.line == null) continue;
      const key = threadKey(thread.path, threadSide(thread), thread.line);
      const list = map.get(key);
      if (list) list.push(thread);
      else map.set(key, [thread]);
    }
    return map;
  }, [threads]);

  const draftsByFile = useMemo(() => {
    const map = new Map<string, LineDraft[]>();
    for (const d of drafts) {
      const list = map.get(d.path);
      if (list) list.push(d);
      else map.set(d.path, [d]);
    }
    return map;
  }, [drafts]);

  // ── Selection ──

  const pickLayout = (next: DiffLayout) => {
    setLayoutState(next);
    setDiffLayout(next);
  };

  const pickWhitespace = (next: boolean) => {
    setHideWhitespaceState(next);
    setIgnoreWhitespace(next);
  };

  const repin = useCallback(
    (id: string, path: string, side: DiffRowSide, line: number) => {
      const ctx = anchorContext(anchorIndex, path, side, line);
      if (!ctx || !headOid) {
        toast.error("That line isn't in the current diff.");
        return;
      }
      updateLineDraft(draftKey, id, {
        path,
        side,
        line,
        startLine: null,
        lineText: ctx.text,
        startLineText: null,
        contextBefore: ctx.contextBefore,
        contextAfter: ctx.contextAfter,
        hunkHeader: ctx.hunk,
        headOidAtDraft: headOid,
        status: "pinned",
        movedFrom: null,
      });
      setRepinId(null);
      toast.success("Note re-anchored");
    },
    [anchorIndex, draftKey, headOid],
  );

  const selectLine = useCallback(
    (path: string, line: DiffLine, side: DiffRowSide, shiftKey: boolean) => {
      const no = side === "LEFT" ? line.oldLine : line.newLine;
      if (no == null) return;
      // Re-pinning an existing note stays available: those notes were
      // written when the host could still take them.
      if (!canDraftLineNotes && !repinId) return;

      if (repinId) {
        repin(repinId, path, side, no);
        return;
      }

      setEditingId(null);
      // A range has to stay on one side and one file — the host has no
      // way to express a note that spans both. Extending one keeps the
      // anchor, and with it whatever has been typed about it.
      if (shiftKey && selection && selection.path === path && selection.side === side) {
        setSelection({
          ...selection,
          start: Math.min(selection.anchor, no),
          end: Math.max(selection.anchor, no),
        });
        return;
      }

      // Anything else starts a different note, or none at all.
      composerDraft.current = "";
      if (
        selection &&
        selection.path === path &&
        selection.side === side &&
        selection.start === no &&
        selection.end === no
      ) {
        setSelection(null); // clicking the same single line again clears it
        return;
      }
      setSelection({ path, side, anchor: no, start: no, end: no });
    },
    [repinId, repin, selection, canDraftLineNotes],
  );

  // Escape clears a selection from anywhere in the tab, not just the
  // textarea — the highlight is the thing that looks stuck.
  useEffect(() => {
    if (!selection && !repinId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setSelection(null);
      composerDraft.current = "";
      setRepinId(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection, repinId]);

  const addNote = useCallback(
    (body: string) => {
      if (!selection) return;
      const { path, side, start, end } = selection;
      const endCtx = anchorContext(anchorIndex, path, side, end);
      if (!endCtx) {
        toast.error("Couldn't work out which line that is.");
        return;
      }
      const startCtx = start !== end ? anchorContext(anchorIndex, path, side, start) : null;
      addLineDraft(draftKey, {
        path,
        side,
        line: end,
        startLine: start !== end ? start : null,
        lineText: endCtx.text,
        startLineText: startCtx?.text ?? null,
        contextBefore: (startCtx ?? endCtx).contextBefore,
        contextAfter: endCtx.contextAfter,
        hunkHeader: endCtx.hunk,
        headOidAtDraft: headOid ?? "",
        body,
      });
      setSelection(null);
      composerDraft.current = "";
    },
    [selection, anchorIndex, draftKey, headOid],
  );

  const commentNow = useCallback(
    (body: string) => {
      if (!selection || !headOid) return;
      const { path, side, start, end } = selection;
      setPosting(true);
      addPrInlineComment(
        cwd,
        prNumber,
        {
          file: path,
          body,
          side,
          line: end,
          start_line: start !== end ? start : null,
        },
        headOid,
      )
        .then(() => {
          setSelection(null);
          composerDraft.current = "";
          onPosted();
          toast.success("Comment posted");
        })
        .catch((err) => toast.error(String(err)))
        .finally(() => setPosting(false));
    },
    [selection, headOid, cwd, prNumber, onPosted],
  );

  /**
   * Every coordinate a rendered row can be addressed by.
   *
   * Split view draws a context line in *both* columns and asks about
   * each side separately, so the side it passes is the whole answer
   * there — anything else would draw the same note twice, once per
   * column. Unified draws that line once and asks about RIGHT only
   * (`sideOf`), but a note written from the split view's LEFT column is
   * stored as (LEFT, oldLine). Matching on the passed side alone meant
   * such a note — and any open composer or live selection on it —
   * rendered nowhere at all after a layout switch, while still
   * submitting perfectly well against the host.
   *
   * So in unified, and only there, a context row answers to both of its
   * coordinates.
   */
  const rowCoords = useCallback(
    (line: DiffLine, side: DiffRowSide): Array<{ side: DiffRowSide; line: number }> => {
      const coords: Array<{ side: DiffRowSide; line: number }> = [];
      const no = side === "LEFT" ? line.oldLine : line.newLine;
      if (no != null) coords.push({ side, line: no });
      if (
        layout === "unified" &&
        line.type === "context" &&
        side === "RIGHT" &&
        line.oldLine != null
      ) {
        coords.push({ side: "LEFT", line: line.oldLine });
      }
      return coords;
    },
    [layout],
  );

  /** The selection/notes behaviour handed to one file's diff view. */
  const selectionFor = useCallback(
    (file: PrDiffFile): DiffSelection => ({
      isSelected: (line, side) => {
        if (!selection || selection.path !== file.path) return false;
        return rowCoords(line, side).some(
          (c) =>
            c.side === selection.side &&
            c.line >= selection.start &&
            c.line <= selection.end,
        );
      },
      onSelect: (line, side, shiftKey) => selectLine(file.path, line, side, shiftKey),
      renderUnder: (line, side) => {
        const coords = rowCoords(line, side);
        if (coords.length === 0) return null;
        const at = (s: DiffRowSide, n: number) =>
          coords.some((c) => c.side === s && c.line === n);

        // An unanchored note's line number points at whatever happens to
        // sit there now, which is not what it was written about. Those
        // live in the panel at the top of the tab instead.
        const notes = (draftsByFile.get(file.path) ?? []).filter(
          (d) => d.status !== "unanchored" && at(d.side, d.line),
        );
        const composerHere =
          selection?.path === file.path &&
          at(selection.side, selection.end) &&
          !editingId;
        const rowThreads = coords.flatMap(
          (c) => threadsByRow.get(threadKey(file.path, c.side, c.line)) ?? [],
        );

        if (!notes.length && !composerHere && !rowThreads.length) return null;

        return (
          <>
            {/* What others already said comes before what you are
                writing, the order you would read it on the host. */}
            {rowThreads.map((thread) => (
              <CodeThread
                key={thread.id}
                thread={thread}
                resolved={resolvedOf(thread)}
                onSendToAgent={onSendToAgent}
                onReply={onReply}
                onSetResolved={toggleResolved}
                suggestionTargetFor={suggestionTargetFor}
              />
            ))}
            {notes.map((note) =>
              editingId === note.id ? (
                <ReviewLineComposer
                  key={note.id}
                  label={rangeLabel(note.startLine, note.line)}
                  initialBody={note.body}
                  onAddToReview={(body) => {
                    updateLineDraft(draftKey, note.id, { body });
                    setEditingId(null);
                  }}
                  onCancel={() => setEditingId(null)}
                />
              ) : (
                <PendingNote
                  key={note.id}
                  note={note}
                  onEdit={() => {
                    setSelection(null);
                    setEditingId(note.id);
                  }}
                  onDelete={() => removeLineDraft(draftKey, note.id)}
                />
              ),
            )}
            {composerHere && (
              <ReviewLineComposer
                label={rangeLabel(
                  selection.start === selection.end ? null : selection.start,
                  selection.end,
                )}
                initialBody={composerDraft.current}
                onBodyChange={(value) => {
                  composerDraft.current = value;
                }}
                busy={posting}
                onAddToReview={addNote}
                onCommentNow={canCommentNow ? commentNow : undefined}
                onCancel={() => {
                  setSelection(null);
                  composerDraft.current = "";
                }}
              />
            )}
          </>
        );
      },
    }),
    [
      selection,
      selectLine,
      rowCoords,
      draftsByFile,
      editingId,
      draftKey,
      posting,
      canDraftLineNotes,
      addNote,
      commentNow,
      canCommentNow,
      threadsByRow,
      resolvedOf,
      toggleResolved,
      onSendToAgent,
      onReply,
      suggestionTargetFor,
    ],
  );

  // ── Render ──

  if (loading && !diffText) {
    return (
      <p className={cn("px-3.5 py-6 text-center text-muted-foreground", tzBody)}>
        Loading the diff…
      </p>
    );
  }

  if (error && !diffText) {
    return (
      <p className={cn("px-3.5 py-6 text-center text-muted-foreground", tzBody)}>
        Couldn't read the diff — {error}
      </p>
    );
  }

  return (
    // No scroll container of its own: the detail surface is one scroll
    // from the header to the action bar, and a diff with its own
    // scrollbar inside it turns "keep reading" into "find the right
    // scrollbar first".
    <div ref={rootRef} className="@container flex flex-1 flex-col">
      <div className="flex items-center gap-1.5 border-b border-border/40 px-3 py-1.5">
        <span className={cn("flex-1 text-muted-foreground", tzMetaNum)}>
          {files.length === 1 ? "1 file" : `${files.length} files`} changed
        </span>
        <div className="flex gap-px rounded-md bg-muted/60 p-0.5" role="radiogroup" aria-label="Diff layout">
          {(["split", "unified"] as const).map((id) => (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={layout === id}
              data-testid={`diff-layout-${id}`}
              onClick={() => pickLayout(id)}
              className={cn(
                "rounded-sm px-2.5 py-1 capitalize transition-colors duration-150",
                tzMetaNum,
                layout === id
                  ? "bg-background font-semibold text-foreground"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {id}
            </button>
          ))}
        </div>
        <button
          type="button"
          aria-pressed={hideWhitespace}
          data-testid="whitespace-toggle"
          onClick={() => pickWhitespace(!hideWhitespace)}
          className={cn(
            "rounded-sm border-0 px-2.5 py-1 transition-colors duration-150",
            tzMetaNum,
            hideWhitespace
              ? "bg-accent-ember/15 font-semibold text-accent-ember"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          Whitespace
        </button>
      </div>

      {showingOld && (
        <div
          data-testid="old-diff-banner"
          className="flex items-center gap-2 border-b border-border/40 bg-muted/40 px-3 py-1.5"
        >
          <span className={cn("flex-1 text-foreground/80", tzBody)}>
            The diff as it was when you wrote these notes — the branch has moved since.
          </span>
          <button type="button" className={btnCard} onClick={() => setOldDiffOid(null)}>
            Back to current
          </button>
        </div>
      )}

      {repinId && (
        <div
          data-testid="repin-banner"
          className="flex items-center gap-2 border-b border-border/40 bg-accent-ember/10 px-3 py-1.5"
        >
          <span className={cn("flex-1 text-foreground/80", tzBody)}>
            Click the line this note belongs to now.
            {unanchored.length > 1 && ` ${unanchored.length - 1} more after this one.`}
          </span>
          <button type="button" className={btnCard} onClick={() => setRepinId(null)}>
            Cancel
          </button>
        </div>
      )}

      {/* A note whose line is gone has nowhere in the diff to sit, and
          a note you can't see is a note you'll submit by accident. They
          come to the top and stay there until they're re-pinned or
          deleted. */}
      {unanchored.length > 0 && !showingOld && (
        <div
          data-testid="unanchored-notes"
          className="border-b border-border/40 bg-muted/20 px-3 py-2"
        >
          <p className={cn("mb-1.5 font-semibold text-status-working", tzMetaNum)}>
            {unanchored.length === 1
              ? "1 note no longer matches a line"
              : `${unanchored.length} notes no longer match a line`}
          </p>
          {unanchored.map((note) => (
            <div key={note.id} className="flex items-start gap-2 py-0.5">
              <span className={cn("shrink-0 font-mono text-muted-foreground", tzMeta)}>
                {note.path}:{note.line}
              </span>
              <span className={cn("min-w-0 flex-1 leading-snug text-foreground/80", tzBody)}>
                {note.body}
              </span>
              <button
                type="button"
                data-testid="repin-note"
                onClick={() => {
                  setRepinId(note.id);
                  scrollToFile(note.path);
                }}
                className={cn(
                  "shrink-0 font-semibold text-accent-ember hover:underline",
                  tzMeta,
                )}
              >
                pick a line
              </button>
              <button
                type="button"
                onClick={() => removeLineDraft(draftKey, note.id)}
                className={cn("shrink-0 text-muted-foreground hover:text-foreground", tzMeta)}
              >
                delete
              </button>
            </div>
          ))}
        </div>
      )}

      <div>
        {files.length === 0 ? (
          <p className={cn("px-3.5 py-6 text-center text-muted-foreground", tzBody)}>
            No file changes in this pull request.
          </p>
        ) : (
          files.map((file) => (
            <ReviewCodeFile
              key={file.path}
              file={file}
              layout={layout}
              hideWhitespace={hideWhitespace}
              viewed={viewed.has(file.path)}
              onToggleViewed={() => {
                // Marking the file you were sent to as viewed is asking
                // for it to fold, so the reveal stops holding it open.
                if (focus?.path === file.path) setFocus(null);
                toggleFileViewed(draftKey, file.path);
              }}
              // So is folding it by hand.
              onCollapse={() => {
                if (focus?.path === file.path) setFocus(null);
              }}
              // Reading a stale snapshot is reading, not reviewing:
              // notes written there would anchor to lines that are gone.
              selection={showingOld ? undefined : selectionFor(file)}
              pendingNotes={(draftsByFile.get(file.path) ?? []).length}
              forceOpen={
                (repinId != null && repinPath === file.path) || focus?.path === file.path
              }
            />
          ))
        )}
      </div>
    </div>
  );
}

export function rangeLabel(startLine: number | null, line: number): string {
  return startLine == null ? `line ${line}` : `lines ${startLine}–${line}`;
}

/**
 * A note you've written but nobody else can see.
 *
 * It sits in the diff at its anchor rather than in a list somewhere,
 * because the question you ask about a pending note is always "what did
 * I say about *this*".
 */
function PendingNote({
  note,
  onEdit,
  onDelete,
}: {
  note: LineDraft;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      data-testid="pending-note"
      data-note-status={note.status}
      className={cn("my-1 rounded-lg bg-accent-ember/10 px-3 py-2", underRowInset)}
    >
      <div className="flex items-center gap-2">
        <span className={cn("font-mono text-accent-ember", tzMeta)}>
          {rangeLabel(note.startLine, note.line)}
        </span>
        {note.status === "moved" && note.movedFrom != null && (
          <span
            data-testid="note-moved-badge"
            className={cn(
              "rounded-sm bg-muted/60 px-1.5 py-0.5 font-mono text-muted-foreground",
              tzEyebrow,
            )}
          >
            moved {note.movedFrom} → {note.line}
          </span>
        )}
        {note.status === "unanchored" && (
          <span
            data-testid="note-unanchored-label"
            className={cn(
              "rounded-sm bg-status-working/15 px-1.5 py-0.5 font-semibold text-status-working",
              tzEyebrow,
            )}
          >
            no longer matches a line
          </span>
        )}
        <span className="flex-1" />
        <button
          type="button"
          onClick={onEdit}
          className={cn("text-muted-foreground hover:text-foreground", tzMeta)}
        >
          edit
        </button>
        <button
          type="button"
          onClick={onDelete}
          className={cn("text-muted-foreground hover:text-foreground", tzMeta)}
        >
          delete
        </button>
      </div>
      <p className={cn("mt-1 whitespace-pre-wrap font-sans leading-snug text-foreground", tzBodyLg)}>
        {note.body}
      </p>
    </div>
  );
}
