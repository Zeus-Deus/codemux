import { ChevronLeft, ChevronRight, Pencil } from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { Button } from "@/components/ui/button";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { cn } from "@/lib/utils";
import type { PermissionRequestItem } from "@/lib/agent-chat/types";

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

/**
 * Composer-attached panel for `request_kind: "user-input"`. The
 * panel lives with the composer (not inline in the transcript) and
 * follows the Claude.ai interaction pattern (one question per page,
 * prev/next arrows, numbered options, always-visible "Something
 * else" free-text row as the last option, keyboard-driven).
 *
 * Layout: outer wrapper matches the Composer's own centering/width so
 * the two visually belong together. The panel does NOT alter the
 * composer itself — branch / worktree / model pickers render exactly
 * as before when the panel is mounted above them.
 */
export function ComposerPendingInputPanel({ item, onSubmit }: Props) {
  const questions = useMemo(
    () => extractQuestions(item.payload),
    [item.payload],
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

  // ------- Selection + answer helpers ---------------------------------

  const togglePick = useCallback(
    (label: string, multiSelect: boolean) => {
      setPicks((prev) => {
        const next = { ...prev };
        const cur = new Set(next[qi] ?? []);
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
        next[qi] = cur;
        return next;
      });
    },
    [qi],
  );

  const questionAnswered = useCallback(
    (i: number): boolean => {
      const q = questions[i];
      if (!q) return false;
      const p = picks[i];
      const free = (otherText[i] ?? "").trim();
      // Typing into "Something else" counts as selecting Other even if
      // the user never explicitly clicked the row — matches Claude.ai's
      // free-text-row behavior.
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
      const free = (otherText[i] ?? "").trim();
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

  const isFirst = qi === 0;
  const isLast = qi === questions.length - 1;

  const goPrev = useCallback(() => {
    setQi((i) => Math.max(0, i - 1));
  }, []);
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

  // ------- Keyboard shortcuts (global, with input-focus guard) --------
  //
  // Listen on the document, but only fire when focus is outside an
  // input / textarea / contenteditable so the composer textarea and
  // our own "Something else" input keep normal typing semantics.

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

      // Digit 1-9 → toggle Nth option on the current question.
      const digit = Number.parseInt(e.key, 10);
      if (!Number.isNaN(digit) && digit >= 1 && digit <= 9) {
        const q = questions[qi];
        if (!q) return;
        if (digit > q.options.length) return;
        const opt = q.options[digit - 1];
        if (!opt) return;
        e.preventDefault();
        togglePickRef.current(opt.label, q.multiSelect);
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
      <div className="w-full px-4 pb-2">
        <div className="mx-auto w-full max-w-[760px]">
          <div className="rounded-[20px] border border-border bg-muted/40 shadow-sm px-4 py-3 text-xs text-muted-foreground">
            AskUserQuestion with no questions.
          </div>
        </div>
      </div>
    );
  }

  const q = questions[qi]!;
  const nextDisabled = !currentAnswered;
  const primaryLabel = isLast ? "Send" : "Next";

  return (
    <div className="w-full px-4 pb-2">
      <div className="mx-auto w-full max-w-[760px]">
        <div className="rounded-[20px] border border-border bg-muted/40 shadow-sm px-4 py-3 space-y-3">
          {/* Header row: optional uppercase header on the left, pagination on the right. */}
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-[0.08em] text-muted-foreground/70">
              {q.header || "Input requested"}
            </span>
            <div className="ml-auto flex items-center gap-1">
              <NavButton
                onClick={goPrev}
                disabled={isFirst}
                aria-label="Previous question"
              >
                <ChevronLeft className="h-3.5 w-3.5" />
              </NavButton>
              <span className="px-1 font-mono text-[10px] tabular-nums text-muted-foreground/70">
                {qi + 1} of {questions.length}
              </span>
              <NavButton
                onClick={goNext}
                disabled={isLast || nextDisabled}
                aria-label="Next question"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </NavButton>
            </div>
          </div>

          {/* Question */}
          <div className="text-sm text-foreground">{q.question}</div>
          {q.multiSelect && (
            <p className="text-[11px] text-muted-foreground/70">
              Select one or more.
            </p>
          )}

          {/* Option rows */}
          <div className="space-y-1">
            {q.options.map((opt, oi) => (
              <OptionRow
                key={`${qi}-${oi}`}
                questionIndex={qi}
                optionIndex={oi}
                shortcut={oi + 1}
                label={opt.label}
                description={opt.description}
                preview={opt.preview}
                checked={picks[qi]?.has(opt.label) ?? false}
                multiSelect={q.multiSelect}
                onToggle={() => togglePick(opt.label, q.multiSelect)}
              />
            ))}

            {/* "Something else" free-text row — always visible. Typing
                into it implicitly selects Other; no explicit click
                required, matching Claude.ai's flow. */}
            <OtherRow
              key={`${qi}-other`}
              questionIndex={qi}
              value={otherText[qi] ?? ""}
              onChange={(val) =>
                setOtherText((prev) => ({ ...prev, [qi]: val }))
              }
              onEnter={advanceOrSubmit}
            />
          </div>

          {/* Footer: keyboard hints + primary action */}
          <div className="flex items-center justify-between gap-3 pt-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[10px] text-muted-foreground/60">
              <span>
                <Kbd>1–9</Kbd> select
              </span>
              <span aria-hidden>·</span>
              <span>
                <Kbd>←</Kbd> <Kbd>→</Kbd> navigate
              </span>
              <span aria-hidden>·</span>
              <span>
                <Kbd>Enter</Kbd> {isLast ? "send" : "next"}
              </span>
            </div>
            <Button
              type="button"
              size="sm"
              className="h-7 px-3 text-xs bg-foreground text-background hover:bg-foreground/90"
              onClick={isLast ? handleSubmit : goNext}
              disabled={submitted || nextDisabled}
            >
              {primaryLabel}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Subcomponents
// ---------------------------------------------------------------------------

function NavButton({
  children,
  disabled,
  onClick,
  ...rest
}: {
  children: React.ReactNode;
  disabled: boolean;
  onClick: () => void;
} & Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, "children" | "disabled" | "onClick">) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "flex h-5 w-5 items-center justify-center rounded text-muted-foreground/70 transition-colors",
        "disabled:opacity-30 disabled:cursor-not-allowed",
        "enabled:hover:bg-muted/60 enabled:hover:text-foreground",
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

interface OptionRowProps {
  questionIndex: number;
  optionIndex: number;
  shortcut: number;
  label: string;
  description: string;
  /** Optional `option.preview` from the SDK. When non-null the row is
   *  wrapped in a HoverCard so the user can preview the underlying
   *  payload before committing to the choice. */
  preview: string | null;
  checked: boolean;
  multiSelect: boolean;
  onToggle: () => void;
}

/**
 * A selectable option row. Native `<input>` stays in the DOM (hidden
 * via `sr-only`) so `role="radio" | "checkbox"` and keyboard a11y keep
 * working for assistive tech; the visible surface is a full-width
 * button-card.
 */
function OptionRow({
  questionIndex,
  optionIndex,
  shortcut,
  label,
  description,
  preview,
  checked,
  multiSelect,
  onToggle,
}: OptionRowProps) {
  const inputId = `aq-${questionIndex}-${optionIndex}`;
  const row = (
    <label
      htmlFor={inputId}
      data-testid={`aq-option-${questionIndex}-${optionIndex}`}
      className={cn(
        "group flex w-full items-start gap-3 rounded-md border px-3 py-2 cursor-pointer transition-colors",
        checked
          ? "border-foreground/30 bg-foreground/[0.04]"
          : "border-border/40 hover:border-border hover:bg-muted/30",
      )}
    >
      <input
        id={inputId}
        type={multiSelect ? "checkbox" : "radio"}
        name={`aq-${questionIndex}`}
        checked={checked}
        onChange={onToggle}
        className="sr-only"
      />
      <kbd
        aria-hidden
        className={cn(
          "mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded font-mono text-[10px] tabular-nums transition-colors",
          checked
            ? "bg-foreground text-background"
            : "bg-muted/50 text-muted-foreground/70 group-hover:bg-muted/70 group-hover:text-muted-foreground",
        )}
      >
        {shortcut}
      </kbd>
      <div className="min-w-0 flex-1">
        <div className="text-sm text-foreground leading-snug">{label}</div>
        {description && (
          <div className="mt-0.5 text-xs text-muted-foreground leading-snug">
            {description}
          </div>
        )}
      </div>
    </label>
  );

  if (!preview) return row;

  return (
    <HoverCard openDelay={300}>
      <HoverCardTrigger asChild>{row}</HoverCardTrigger>
      <HoverCardContent
        align="start"
        side="right"
        className="w-80"
        data-testid={`aq-option-preview-${questionIndex}-${optionIndex}`}
      >
        <p className="mb-1 text-[10px] uppercase tracking-[0.08em] text-muted-foreground/70">
          Preview
        </p>
        <pre className="whitespace-pre-wrap break-words font-mono text-[11.5px] leading-5 text-foreground">
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
 * Always-visible "Something else" row. Pencil icon on the left, inline
 * text input on the right. No radio — typing auto-selects Other, per
 * Claude.ai's flow. Enter inside the field advances/submits, so the
 * user can answer the whole panel without leaving the keyboard.
 */
function OtherRow({ questionIndex, value, onChange, onEnter }: OtherRowProps) {
  const hasText = value.trim().length > 0;
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      onEnter();
    }
  };
  return (
    <div
      className={cn(
        "flex w-full items-center gap-3 rounded-md border px-3 py-2 transition-colors",
        hasText
          ? "border-foreground/30 bg-foreground/[0.04]"
          : "border-border/40",
      )}
    >
      <Pencil
        aria-hidden
        className="h-3.5 w-3.5 shrink-0 text-muted-foreground/70"
      />
      <input
        id={`aq-${questionIndex}-other`}
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Something else…"
        className={cn(
          "flex-1 bg-transparent text-sm text-foreground placeholder:text-muted-foreground/60 outline-none",
        )}
      />
    </div>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
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
    out.push({ header, question, multiSelect, options });
  }
  return out;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
