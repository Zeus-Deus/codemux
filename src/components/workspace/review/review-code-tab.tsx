import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { addPrInlineComment } from "@/tauri/commands";
import { anchorContext, indexDiffRows } from "@/lib/pr-anchor";
import { splitDiffFiles, type PrDiffFile } from "@/lib/pr-diff";
import type { DiffLine } from "@/lib/diff-parser";
import type { DiffRowSide, DiffSelection } from "@/components/diff/diff-row";
import { ReviewCodeFile } from "./review-code-file";
import { ReviewLineComposer } from "./review-line-composer";
import { btnCard } from "./review-ui";
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
export interface CodeTabIntent {
  kind: "repin" | "old-diff";
  nonce: number;
}

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
  const rootRef = useRef<HTMLDivElement>(null);

  const unanchored = useMemo(
    () => drafts.filter((d) => d.status === "unanchored"),
    [drafts],
  );
  const repinPath = repinId ? (drafts.find((d) => d.id === repinId)?.path ?? null) : null;

  // ── Intents from the drift notice ──
  const handledNonce = useRef(0);
  useEffect(() => {
    if (!intent || intent.nonce === handledNonce.current) return;
    handledNonce.current = intent.nonce;
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
  }, [intent, drafts, draftKey]);

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
      setSelection((prev) => {
        // A range has to stay on one side and one file — the host has no
        // way to express a note that spans both.
        if (shiftKey && prev && prev.path === path && prev.side === side) {
          return {
            ...prev,
            start: Math.min(prev.anchor, no),
            end: Math.max(prev.anchor, no),
          };
        }
        if (prev && prev.path === path && prev.side === side && prev.start === no && prev.end === no) {
          return null; // clicking the same single line again clears it
        }
        return { path, side, anchor: no, start: no, end: no };
      });
    },
    [repinId, repin],
  );

  // Escape clears a selection from anywhere in the tab, not just the
  // textarea — the highlight is the thing that looks stuck.
  useEffect(() => {
    if (!selection && !repinId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setSelection(null);
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
          onPosted();
          toast.success("Comment posted");
        })
        .catch((err) => toast.error(String(err)))
        .finally(() => setPosting(false));
    },
    [selection, headOid, cwd, prNumber, onPosted],
  );

  /** The selection/notes behaviour handed to one file's diff view. */
  const selectionFor = useCallback(
    (file: PrDiffFile): DiffSelection => ({
      isSelected: (line, side) => {
        if (!selection || selection.path !== file.path || selection.side !== side) {
          return false;
        }
        const no = side === "LEFT" ? line.oldLine : line.newLine;
        return no != null && no >= selection.start && no <= selection.end;
      },
      onSelect: (line, side, shiftKey) => selectLine(file.path, line, side, shiftKey),
      renderUnder: (line, side) => {
        const no = side === "LEFT" ? line.oldLine : line.newLine;
        if (no == null) return null;

        // An unanchored note's line number points at whatever happens to
        // sit there now, which is not what it was written about. Those
        // live in the panel at the top of the tab instead.
        const notes = (draftsByFile.get(file.path) ?? []).filter(
          (d) => d.status !== "unanchored" && d.side === side && d.line === no,
        );
        const composerHere =
          selection?.path === file.path &&
          selection.side === side &&
          selection.end === no &&
          !editingId;

        if (!notes.length && !composerHere) return null;

        return (
          <>
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
                busy={posting}
                onAddToReview={addNote}
                onCommentNow={canCommentNow ? commentNow : undefined}
                onCancel={() => setSelection(null)}
              />
            )}
          </>
        );
      },
    }),
    [
      selection,
      selectLine,
      draftsByFile,
      editingId,
      draftKey,
      posting,
      canDraftLineNotes,
      addNote,
      commentNow,
      canCommentNow,
    ],
  );

  // ── Render ──

  if (loading && !diffText) {
    return (
      <p className="px-3.5 py-6 text-center text-[11px] text-muted-foreground">
        Loading the diff…
      </p>
    );
  }

  if (error && !diffText) {
    return (
      <p className="px-3.5 py-6 text-center text-[11px] text-muted-foreground">
        Couldn't read the diff — {error}
      </p>
    );
  }

  return (
    // No scroll container of its own: the detail surface is one scroll
    // from the header to the action bar, and a diff with its own
    // scrollbar inside it turns "keep reading" into "find the right
    // scrollbar first".
    <div ref={rootRef} className="flex flex-1 flex-col">
      <div className="flex items-center gap-1.5 border-b border-border/40 px-3 py-1.5">
        <span className="flex-1 text-[10.5px] text-muted-foreground">
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
                "rounded px-2 py-0.5 text-[10.5px] capitalize transition-colors",
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
            "rounded border-0 px-2 py-0.5 text-[10.5px] transition-colors",
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
          <span className="flex-1 text-[11px] text-foreground/80">
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
          <span className="flex-1 text-[11px] text-foreground/80">
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
          <p className="mb-1 text-[10.5px] font-semibold text-status-working">
            {unanchored.length === 1
              ? "1 note no longer matches a line"
              : `${unanchored.length} notes no longer match a line`}
          </p>
          {unanchored.map((note) => (
            <div key={note.id} className="flex items-start gap-2 py-0.5">
              <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                {note.path}:{note.line}
              </span>
              <span className="min-w-0 flex-1 text-[11px] leading-snug text-foreground/80">
                {note.body}
              </span>
              <button
                type="button"
                data-testid="repin-note"
                onClick={() => {
                  setRepinId(note.id);
                  scrollToFile(note.path);
                }}
                className="shrink-0 text-[10px] font-semibold text-accent-ember hover:underline"
              >
                pick a line
              </button>
              <button
                type="button"
                onClick={() => removeLineDraft(draftKey, note.id)}
                className="shrink-0 text-[10px] text-muted-foreground hover:text-foreground"
              >
                delete
              </button>
            </div>
          ))}
        </div>
      )}

      <div>
        {files.length === 0 ? (
          <p className="px-3.5 py-6 text-center text-[11px] text-muted-foreground">
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
              onToggleViewed={() => toggleFileViewed(draftKey, file.path)}
              // Reading a stale snapshot is reading, not reviewing:
              // notes written there would anchor to lines that are gone.
              selection={showingOld ? undefined : selectionFor(file)}
              pendingNotes={(draftsByFile.get(file.path) ?? []).length}
              forceOpen={repinId != null && repinPath === file.path}
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
      className="my-1 ml-[72px] mr-3 rounded-lg bg-accent-ember/10 px-2.5 py-1.5"
    >
      <div className="flex items-center gap-2">
        <span className="font-mono text-[10px] text-accent-ember">
          {rangeLabel(note.startLine, note.line)}
        </span>
        {note.status === "moved" && note.movedFrom != null && (
          <span
            data-testid="note-moved-badge"
            className="rounded bg-muted/60 px-1.5 py-px font-mono text-[9.5px] text-muted-foreground"
          >
            moved {note.movedFrom} → {note.line}
          </span>
        )}
        {note.status === "unanchored" && (
          <span
            data-testid="note-unanchored-label"
            className="rounded bg-status-working/15 px-1.5 py-px text-[9.5px] font-semibold text-status-working"
          >
            no longer matches a line
          </span>
        )}
        <span className="flex-1" />
        <button
          type="button"
          onClick={onEdit}
          className="text-[10px] text-muted-foreground hover:text-foreground"
        >
          edit
        </button>
        <button
          type="button"
          onClick={onDelete}
          className="text-[10px] text-muted-foreground hover:text-foreground"
        >
          delete
        </button>
      </div>
      <p className="mt-0.5 whitespace-pre-wrap font-sans text-[11.5px] leading-snug text-foreground">
        {note.body}
      </p>
    </div>
  );
}
