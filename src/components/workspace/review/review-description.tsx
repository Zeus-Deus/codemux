import { useEffect, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { MarkdownRendered } from "@/components/editor/MarkdownRendered";
import { updatePullRequest } from "@/tauri/commands";
import {
  btnCard,
  btnCardXs,
  btnEmberSolid,
  tzBody,
  tzEyebrow,
  tzMeta,
} from "./review-ui";
import {
  clearDescriptionDraft,
  getDescriptionDraft,
  getDescriptionFold,
  setDescriptionDraft,
  setDescriptionFold,
  type DraftKey,
} from "./pr-drafts";

/** Past this many lines a description stops being context and starts
 *  being the page. Fold, and remember the fold per PR. */
const FOLD_AFTER_LINES = 40;

interface Props {
  body: string | null;
  cwd: string;
  prNumber: number;
  draftKey: DraftKey;
  /** False on hosts with no adapter: the `edit` affordance is simply
   *  not drawn rather than drawn dead. */
  canEdit: boolean;
  /** Merged/closed PRs are a record, not a form. */
  readOnly: boolean;
  onSaved: () => void;
}

export function ReviewDescription({
  body,
  cwd,
  prNumber,
  draftKey,
  canEdit,
  readOnly,
  onSaved,
}: Props) {
  const text = body ?? "";
  const lineCount = text ? text.split("\n").length : 0;
  const foldable = lineCount > FOLD_AFTER_LINES;

  const [folded, setFolded] = useState(
    () => getDescriptionFold(draftKey) ?? foldable,
  );
  // `undefined` = not editing. An empty string is a description the user
  // deliberately cleared and must survive a refetch like any other text.
  const [draft, setDraft] = useState<string | undefined>(() =>
    getDescriptionDraft(draftKey),
  );
  const [saving, setSaving] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Re-key when the panel switches PR: pick up that PR's own scratch
  // state rather than carrying this one's into it.
  const lastKey = useRef(draftKey);
  useEffect(() => {
    if (lastKey.current === draftKey) return;
    lastKey.current = draftKey;
    setDraft(getDescriptionDraft(draftKey));
    setFolded(getDescriptionFold(draftKey) ?? foldable);
  }, [draftKey, foldable]);

  const editing = draft !== undefined;

  const startEdit = () => {
    const initial = getDescriptionDraft(draftKey) ?? text;
    setDraft(initial);
    setDescriptionDraft(draftKey, initial);
    // Editing a folded description would hide half of what you're
    // editing.
    setFolded(false);
    setDescriptionFold(draftKey, false);
  };

  const onChange = (value: string) => {
    setDraft(value);
    setDescriptionDraft(draftKey, value);
  };

  const cancel = () => {
    // The only two ways typed text goes away: saved, or explicitly
    // discarded here.
    clearDescriptionDraft(draftKey);
    setDraft(undefined);
  };

  const save = async () => {
    if (draft === undefined || saving) return;
    setSaving(true);
    try {
      await updatePullRequest(cwd, prNumber, null, draft);
      clearDescriptionDraft(draftKey);
      setDraft(undefined);
      onSaved();
      toast.success("Description updated");
    } catch (err) {
      // Failure keeps the draft exactly as typed.
      toast.error(String(err));
    } finally {
      setSaving(false);
    }
  };

  const toggleFold = () => {
    const next = !folded;
    setFolded(next);
    setDescriptionFold(draftKey, next);
  };

  return (
    <div className="flex flex-col gap-1.5" data-testid="review-description">
      <div className="flex items-center gap-1.5">
        <span
          className={cn(
            "font-mono font-semibold uppercase tracking-[0.07em] text-muted-foreground",
            tzEyebrow,
          )}
        >
          Description
        </span>
        {foldable && !editing && (
          <button
            type="button"
            onClick={toggleFold}
            aria-expanded={!folded}
            aria-label={folded ? "Unfold description" : "Fold description"}
            className="text-muted-foreground transition-colors hover:text-foreground"
          >
            <ChevronDown
              className={cn("size-3 transition-transform", folded && "-rotate-90")}
            />
          </button>
        )}
        <span className="h-px flex-1 bg-border/40" />
        {canEdit && !readOnly && !editing && (
          <button
            type="button"
            onClick={startEdit}
            className={cn(
              "text-muted-foreground transition-colors hover:text-foreground",
              tzMeta,
            )}
            data-testid="edit-description"
          >
            edit
          </button>
        )}
      </div>

      {editing ? (
        <div className="flex flex-col gap-1.5">
          <textarea
            ref={textareaRef}
            autoFocus
            value={draft}
            onChange={(e) => onChange(e.target.value)}
            rows={10}
            className={cn(
              "w-full resize-y rounded-md border-0 bg-muted/40 px-2 py-2 font-mono leading-relaxed text-foreground outline-none focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
              tzBody,
            )}
            data-testid="description-editor"
          />
          <div className="flex items-center gap-1.5">
            <button
              type="button"
              className={btnEmberSolid}
              onClick={() => void save()}
              data-testid="description-save"
            >
              {saving ? "Saving" : "Save"}
            </button>
            <button type="button" className={btnCard} onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      ) : text ? (
        <div className="flex flex-col items-start">
          <div className={cn("w-full", folded && "relative max-h-40 overflow-hidden")}>
            <MarkdownRendered content={text} inline />
            {folded && (
              // Scrim only — the last visible line fades out instead of
              // being sliced through. The control below is the control.
              <div
                aria-hidden
                className="pointer-events-none absolute inset-x-0 bottom-0 h-12 bg-gradient-to-t from-background via-background/80 to-transparent"
              />
            )}
          </div>
          {foldable && (
            <button
              type="button"
              onClick={toggleFold}
              aria-expanded={!folded}
              className={cn(
                btnCardXs,
                "mt-1.5 text-muted-foreground hover:text-foreground",
              )}
              data-testid="description-fold"
            >
              <ChevronDown
                className={cn("size-3 transition-transform", !folded && "rotate-180")}
              />
              {folded ? `Show all ${lineCount} lines` : "Show less"}
            </button>
          )}
        </div>
      ) : (
        <p className={cn("text-muted-foreground", tzBody)}>No description.</p>
      )}
    </div>
  );
}
