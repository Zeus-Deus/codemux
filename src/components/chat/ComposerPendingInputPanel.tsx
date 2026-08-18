import { ChevronLeft, Pencil } from "lucide-react";
import {
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireChoice,
  QuestionnaireChoiceDescription,
  QuestionnaireChoices,
  QuestionnaireDescription,
  QuestionnaireError,
  QuestionnaireInput,
  QuestionnaireItem,
  QuestionnaireNext,
  QuestionnairePrevious,
  QuestionnaireProgress,
  QuestionnaireSubmit,
  QuestionnaireTitle,
} from "@/components/ui/questionnaire";
import { cn } from "@/lib/utils";

import type { PermissionRequestItem } from "@/lib/agent-chat/types";

import { CHAT_COLUMN_INNER, CHAT_COLUMN_OUTER } from "./chat-column";

/** Matches the Claude Agent SDK's `AskUserQuestionOutput` (sdk-tools.d.ts):
 *  `answers` is an object keyed by question text, value is the chosen
 *  option label — or a comma-joined list for multiSelect. `questions`
 *  is echoed back verbatim from the original tool_use input so the CLI
 *  can assemble the tool_result. Anything else (array of `{label}`,
 *  synthetic ids, indices) is surfaced to Claude as opaque tokens that
 *  don't match any advertised option label. */
export interface AskUserQuestionOutput {
  questions: unknown;
  answers: Record<string, string>;
}

interface Props {
  /** Must be `request_kind === "user-input"` and in `pending` state.
   *  Non-pending items should render as a tiny inline marker in the
   *  transcript, not mount this panel. */
  item: PermissionRequestItem;
  /** Parent wraps the request_id + `decision: "allow"` shape — this
   *  component produces the full SDK-shaped `updated_input` payload. */
  onSubmit: (output: AskUserQuestionOutput) => void | Promise<void>;
}

const OTHER_LABEL = "Other";

/** Questionnaire items are addressed by *name*, and question text is not
 *  unique (the same prompt can legitimately repeat across questions), so
 *  the index is the identity and `q-<i>` is its wire name. */
const ITEM_NAME_PREFIX = "q-";
const itemNameFor = (index: number) => `${ITEM_NAME_PREFIX}${index}`;
function itemIndexFor(name: string): number {
  const parsed = Number.parseInt(name.slice(ITEM_NAME_PREFIX.length), 10);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * Composer-attached panel for `request_kind: "user-input"`. The
 * panel lives with the composer (not inline in the transcript) and
 * follows the one-question-per-page interaction pattern (prev/next
 * paging, numbered option shortcuts, an always-visible "Something
 * else" free-text row, keyboard-driven).
 *
 * The interior is the shadcn Questionnaire component; the panel keeps
 * the composer-docked wrapper (chat column rails + rounded card) so the
 * two visually belong together. The panel does NOT alter the composer
 * itself — branch / worktree / model pickers render exactly as before
 * when the panel is mounted above them.
 *
 * Every question renders at once (the primitive hides the inactive ones
 * with `hidden` + `inert`); `qi` is the single source of truth for which
 * one is active and is wired to the primitive through the controlled
 * `item` / `onItemChange` pair.
 */
export function ComposerPendingInputPanel({ item, onSubmit }: Props) {
  const questions = useMemo(
    () => extractQuestions(item.payload),
    [item.payload],
  );

  /** Root.items drives paging, validation *and* the auto-rendered
   *  shortcut chips: the primitive maps `shortcuts="numbers"` onto each
   *  item's `choices` in order, keyed by choice value. The values must
   *  therefore be the option labels we actually render, or the chips
   *  silently disappear and dev-mode logs a mismatch warning. */
  const rootItems = useMemo(
    () =>
      questions.map((q, i) => ({
        name: itemNameFor(i),
        required: true,
        choices: q.options.map((o) => ({ value: o.label })),
      })),
    [questions],
  );

  const [qi, setQi] = useState(0);
  const [picks, setPicks] = useState<Record<number, Set<string>>>(() => {
    const init: Record<number, Set<string>> = {};
    questions.forEach((_, i) => {
      init[i] = new Set();
    });
    return init;
  });
  const [otherText, setOtherText] = useState<Record<number, string>>({});
  const [submitted, setSubmitted] = useState(false);
  const submittedRef = useRef(false);
  const formRef = useRef<HTMLFormElement | null>(null);

  // ------- Selection + answer helpers ---------------------------------

  const togglePick = useCallback(
    (index: number, label: string, multiSelect: boolean) => {
      setPicks((prev) => {
        const next = { ...prev };
        const cur = new Set(next[index] ?? []);
        if (multiSelect) {
          if (cur.has(label)) cur.delete(label);
          else cur.add(label);
        } else {
          // Single-select. Re-clicking the already-picked radio is a
          // no-op (standard radio behavior).
          if (cur.has(label) && cur.size === 1) return prev;
          cur.clear();
          cur.add(label);
        }
        next[index] = cur;
        return next;
      });
    },
    [],
  );

  const questionAnswered = useCallback(
    (i: number): boolean => {
      const q = questions[i];
      if (!q) return false;
      const p = picks[i];
      const free = q.allowOther ? (otherText[i] ?? "").trim() : "";
      // Typing into "Something else" counts as selecting Other even if
      // the user never explicitly clicked the row — the free-text row is
      // an answer in its own right.
      if (free.length > 0) return true;
      if (!p || p.size === 0) return false;
      // If Other is the only pick but no free text, the question isn't
      // considered answered yet.
      if (p.size === 1 && p.has(OTHER_LABEL)) return false;
      return true;
    },
    [questions, picks, otherText],
  );

  const allAnswered = useMemo(
    () => questions.every((_, i) => questionAnswered(i)),
    [questions, questionAnswered],
  );
  const currentAnswered = questionAnswered(qi);

  const collectAnswers = useCallback((): AskUserQuestionOutput => {
    // Build the `answers` map keyed by question text per
    // AskUserQuestionOutput. For multiSelect the SDK documents
    // "multi-select answers are comma-separated"; we use ", " to match
    // the CLI's own formatting. Free text from the "Something else" row
    // is surfaced as the actual answer value (for single-select that
    // replaces the "Other" label; for multi-select it's appended as an
    // extra label). This is the user's real answer — keying it under a
    // synthetic "Other" label would hide it from the model.
    const answers: Record<string, string> = {};
    questions.forEach((q, i) => {
      const p = picks[i] ?? new Set<string>();
      const free = q.allowOther ? (otherText[i] ?? "").trim() : "";
      if (q.multiSelect) {
        const labels = [...p].filter((l) => l !== OTHER_LABEL);
        if (free) labels.push(free);
        answers[q.question] = labels.join(", ");
      } else if (free) {
        answers[q.question] = free;
      } else {
        const [picked] = p;
        answers[q.question] = picked ?? "";
      }
    });
    // Echo the raw questions JSON from the original tool_use input so
    // the CLI sees a well-formed AskUserQuestionOutput it can relay to
    // Claude verbatim.
    const rawQuestions = isRecord(item.payload)
      ? (item.payload as Record<string, unknown>)["questions"] ?? []
      : [];
    return { questions: rawQuestions, answers };
  }, [questions, picks, otherText, item.payload]);

  // ------- Navigation -------------------------------------------------

  const isLast = qi === questions.length - 1;

  const goNext = useCallback(() => {
    setQi((i) => Math.min(questions.length - 1, i + 1));
  }, [questions.length]);

  const handleSubmit = useCallback(async () => {
    if (submittedRef.current || submitted || !allAnswered) return;
    submittedRef.current = true;
    setSubmitted(true);
    try {
      await onSubmit(collectAnswers());
    } catch {
      submittedRef.current = false;
      setSubmitted(false);
    }
  }, [submitted, allAnswered, collectAnswers, onSubmit]);

  const advanceOrSubmit = useCallback(() => {
    if (!currentAnswered) return;
    if (isLast) {
      void handleSubmit();
    } else {
      goNext();
    }
  }, [currentAnswered, isLast, handleSubmit, goNext]);

  /** The primitive owns submission: both the Submit button (`type=submit`)
   *  and its Cmd/Ctrl+Enter path funnel through the form. We never let the
   *  browser navigate — the payload is assembled from our own state. */
  const handleFormSubmit = useCallback(
    (e: FormEvent<HTMLFormElement>) => {
      e.preventDefault();
      void handleSubmit();
    },
    [handleSubmit],
  );

  const handleItemChange = useCallback((name: string) => {
    setQi(itemIndexFor(name));
  }, []);

  // ------- Keyboard shortcuts (global, with input-focus guard) --------
  //
  // The primitive already handles digits / arrows / Enter, but only for
  // events that originate *inside* its form. This document-level layer
  // keeps the panel drivable when nothing (or the composer) has focus.
  // It bails on text-entry targets and on anything inside the form, so
  // the two layers never both handle the same keystroke.

  const advanceOrSubmitRef = useRef(advanceOrSubmit);
  advanceOrSubmitRef.current = advanceOrSubmit;
  const togglePickRef = useRef(togglePick);
  togglePickRef.current = togglePick;

  useEffect(() => {
    if (item.resolution.state !== "pending" || questions.length === 0) return;

    const handler = (e: globalThis.KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const target = e.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLSelectElement
      ) {
        return;
      }
      if (
        target instanceof HTMLElement &&
        target.closest('[contenteditable]:not([contenteditable="false"])')
      ) {
        return;
      }
      // Inside the questionnaire form the primitive's own key handling
      // is authoritative — bail so a keystroke isn't applied twice.
      if (target instanceof Node && formRef.current?.contains(target)) return;

      // Digit 1-9 → toggle Nth option on the current question.
      const digit = Number.parseInt(e.key, 10);
      if (!Number.isNaN(digit) && digit >= 1 && digit <= 9) {
        const q = questions[qi];
        if (!q) return;
        if (digit > q.options.length) return;
        const opt = q.options[digit - 1];
        if (!opt) return;
        e.preventDefault();
        togglePickRef.current(qi, opt.label, q.multiSelect);
        return;
      }

      if (e.key === "ArrowLeft") {
        e.preventDefault();
        setQi((i) => Math.max(0, i - 1));
        return;
      }
      if (e.key === "ArrowRight") {
        e.preventDefault();
        setQi((i) => Math.min(questions.length - 1, i + 1));
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        advanceOrSubmitRef.current();
        return;
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [item.resolution.state, questions, qi]);

  // ------- Render -----------------------------------------------------

  if (item.resolution.state !== "pending") return null;

  if (questions.length === 0) {
    return (
      <div className={cn(CHAT_COLUMN_OUTER, "pb-2")}>
        <div className={CHAT_COLUMN_INNER}>
          <div className="rounded-[20px] border border-border bg-muted/40 shadow-sm px-4 py-3 text-xs text-muted-foreground">
            AskUserQuestion with no questions.
          </div>
        </div>
      </div>
    );
  }

  const active = questions[qi]!;
  const nextDisabled = !currentAnswered;

  return (
    <div className={cn(CHAT_COLUMN_OUTER, "pb-2")}>
      <div className={CHAT_COLUMN_INNER}>
        <div className="rounded-[20px] border border-border bg-muted/40 shadow-sm px-4 py-3">
          <Questionnaire
            ref={formRef}
            items={rootItems}
            item={itemNameFor(qi)}
            onItemChange={handleItemChange}
            onSubmit={handleFormSubmit}
            shortcuts="numbers"
            className="gap-3"
          >
            {/* Header row: optional uppercase header on the left,
                compact pagination on the right. Rendered once, from the
                active question — every Item is mounted at all times, so
                a per-item eyebrow would duplicate it. */}
            <div className="flex items-center gap-2">
              <span className="text-[10px] uppercase tracking-[0.08em] text-muted-foreground/70">
                {active.header || "Input requested"}
              </span>
              <QuestionnaireProgress className="ml-auto min-w-0 font-mono text-[10px] text-muted-foreground/70">
                {qi + 1} of {questions.length}
              </QuestionnaireProgress>
            </div>

            {questions.map((q, i) => (
              <QuestionnaireItem
                key={i}
                name={itemNameFor(i)}
                multiple={q.multiSelect}
                required
                className="gap-2"
              >
                <QuestionnaireTitle className="text-sm [&:not(:has(~[data-slot=questionnaire-description]))]:mb-0">
                  {q.question}
                </QuestionnaireTitle>
                {q.multiSelect && (
                  <QuestionnaireDescription className="text-[11px] text-muted-foreground/70">
                    Select one or more.
                  </QuestionnaireDescription>
                )}

                <QuestionnaireChoices className="gap-1.5">
                  {q.options.map((opt, oi) => (
                    <OptionChoice
                      key={`${i}-${oi}`}
                      questionIndex={i}
                      optionIndex={oi}
                      label={opt.label}
                      description={opt.description}
                      preview={opt.preview}
                      checked={picks[i]?.has(opt.label) ?? false}
                      onToggle={() => togglePick(i, opt.label, q.multiSelect)}
                    />
                  ))}
                </QuestionnaireChoices>

                {/* Providers may restrict answers to advertised choices.
                    When free text is allowed, typing here implicitly answers
                    the question without an explicit selection. */}
                {q.allowOther && (
                  <OtherRow
                    questionIndex={i}
                    value={otherText[i] ?? ""}
                    onChange={(val) =>
                      setOtherText((prev) => ({ ...prev, [i]: val }))
                    }
                    onEnter={advanceOrSubmit}
                  />
                )}

                <QuestionnaireError className="text-[11px]" />
              </QuestionnaireItem>
            ))}

            <QuestionnaireActions className="min-h-0 gap-2 sm:min-h-0">
              {/* Column 1 of the actions grid holds the back chevron plus
                  the keyboard hints; the numbered chips on each row already
                  advertise 1-9, so the hint only covers the arrow / Enter
                  layer that has no on-screen affordance. */}
              <div className="col-start-1 row-start-1 flex min-w-0 items-center gap-2">
                <QuestionnairePrevious
                  aria-label="Previous question"
                  size="sm"
                  variant="ghost"
                  className="h-7 min-h-0 w-7 shrink-0 px-0 text-muted-foreground/70 sm:min-h-0"
                >
                  <ChevronLeft className="size-3.5" />
                </QuestionnairePrevious>
                <div
                  data-testid="aq-hints"
                  className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[10px] text-muted-foreground/60"
                >
                  <span>
                    <Kbd>←</Kbd> <Kbd>→</Kbd> navigate
                  </span>
                  <span aria-hidden>·</span>
                  <span>
                    <Kbd>Enter</Kbd> {isLast ? "send" : "next"}
                  </span>
                </div>
              </div>
              <QuestionnaireNext
                size="sm"
                disabled={nextDisabled}
                className="h-7 min-h-0 px-3 text-xs sm:min-h-0"
              >
                Next
              </QuestionnaireNext>
              <QuestionnaireSubmit
                size="sm"
                disabled={submitted || nextDisabled}
                className="h-7 min-h-0 px-3 text-xs sm:min-h-0"
              >
                Send
              </QuestionnaireSubmit>
            </QuestionnaireActions>
          </Questionnaire>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Subcomponents
// ---------------------------------------------------------------------------

interface OptionChoiceProps {
  questionIndex: number;
  optionIndex: number;
  label: string;
  description: string;
  /** Optional `option.preview` from the SDK. When non-null the row is
   *  wrapped in a HoverCard so the user can preview the underlying
   *  payload before committing to the choice. */
  preview: string | null;
  checked: boolean;
  onToggle: () => void;
}

/**
 * A selectable option row. The primitive renders a `<label>` wrapping a
 * positioned native radio/checkbox (type comes from the Item's
 * `multiple`), so role semantics, focus-visible rings and the numbered
 * shortcut chip all come for free.
 */
function OptionChoice({
  questionIndex,
  optionIndex,
  label,
  description,
  preview,
  checked,
  onToggle,
}: OptionChoiceProps) {
  const choice = (
    <QuestionnaireChoice
      value={label}
      checked={checked}
      onChange={onToggle}
      data-testid={`aq-option-${questionIndex}-${optionIndex}`}
      className="min-h-0 gap-2 rounded-md px-2.5 py-1.5"
    >
      <span className="text-sm leading-snug text-foreground">{label}</span>
      {/* The registry's description slot: besides the muted styling it is
          what the indicator / shortcut chip alignment classes key off
          (`group-has-data-[slot=questionnaire-choice-description]`), so a
          raw span here would misalign both by a fraction of a spacing unit. */}
      {description && (
        <QuestionnaireChoiceDescription className="text-xs leading-snug">
          {description}
        </QuestionnaireChoiceDescription>
      )}
    </QuestionnaireChoice>
  );

  if (!preview) return choice;

  return (
    <HoverCard openDelay={300}>
      {/* `asChild` clones the Choice element: the primitive merges the
          extra props (and the ref) straight through onto its `<label>`,
          so the hover target is the whole row. */}
      <HoverCardTrigger asChild>{choice}</HoverCardTrigger>
      <HoverCardContent
        align="start"
        side="right"
        className="w-80"
        data-testid={`aq-option-preview-${questionIndex}-${optionIndex}`}
      >
        <p className="mb-1 text-[10px] uppercase tracking-[0.08em] text-muted-foreground/70">
          Preview
        </p>
        <pre className="whitespace-pre-wrap break-words font-mono text-[12px] leading-5 text-foreground">
          {preview}
        </pre>
      </HoverCardContent>
    </HoverCard>
  );
}

interface OtherRowProps {
  questionIndex: number;
  value: string;
  onChange: (next: string) => void;
  onEnter: () => void;
}

/**
 * Always-visible "Something else" row. Pencil affordance on the left,
 * inline text input on the right. No radio — typing auto-answers the
 * question. Enter inside the field advances/submits so the user can
 * answer the whole panel without leaving the keyboard; the explicit
 * handler (rather than the primitive's native Enter path) keeps that
 * working even when a choice is also selected.
 */
function OtherRow({ questionIndex, value, onChange, onEnter }: OtherRowProps) {
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter") return;
    // The primitive's form-level handler bails on `defaultPrevented`, so
    // this replaces (never duplicates) its Enter behavior. Shift+Enter is
    // swallowed rather than forwarded: it has always been inert in this
    // field, and letting it reach the form would advance/submit.
    e.preventDefault();
    if (e.shiftKey) return;
    onEnter();
  };
  return (
    <div className="relative flex w-full items-center">
      <QuestionnaireInput
        id={`aq-${questionIndex}-other`}
        data-testid={`aq-other-${questionIndex}`}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Something else…"
        className="h-8 min-h-0 rounded-md ps-8 text-sm sm:min-h-0 md:text-sm"
      />
      <Pencil
        aria-hidden
        className="pointer-events-none absolute start-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground/70"
      />
    </div>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded bg-muted/60 px-1 py-[1px] font-mono text-[10px] text-muted-foreground/80">
      {children}
    </kbd>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface Question {
  header: string;
  question: string;
  multiSelect: boolean;
  allowOther: boolean;
  options: { label: string; description: string; preview: string | null }[];
}

function extractQuestions(payload: unknown): Question[] {
  if (!isRecord(payload)) return [];
  const raw = payload["questions"];
  if (!Array.isArray(raw)) return [];
  const out: Question[] = [];
  for (const q of raw) {
    if (!isRecord(q)) continue;
    const question =
      typeof q["question"] === "string" ? (q["question"] as string) : "";
    if (!question) continue;
    const header = typeof q["header"] === "string" ? (q["header"] as string) : "";
    const multiSelect = q["multiSelect"] === true;
    const allowOther = q["allowOther"] !== false;
    const optsRaw = q["options"];
    const options: Question["options"] = [];
    if (Array.isArray(optsRaw)) {
      for (const o of optsRaw) {
        if (!isRecord(o)) continue;
        const label =
          typeof o["label"] === "string" ? (o["label"] as string) : "";
        if (!label) continue;
        const description =
          typeof o["description"] === "string"
            ? (o["description"] as string)
            : "";
        const preview =
          typeof o["preview"] === "string" && (o["preview"] as string).length > 0
            ? (o["preview"] as string)
            : null;
        options.push({ label, description, preview });
      }
    }
    out.push({ header, question, multiSelect, allowOther, options });
  }
  return out;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
