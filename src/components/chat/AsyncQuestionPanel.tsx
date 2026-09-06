import {
  MessageCircleQuestion,
  X,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import type { AsyncQuestionItem } from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { agentChatAnswerQuestion, type QuestionAction } from "@/tauri/commands";
import { QuestionForm, type Question } from "./QuestionForm";
import { CHAT_COLUMN_INNER, CHAT_COLUMN_OUTER } from "./chat-column";
import { cn } from "@/lib/utils";

const draftKey = (threadId: string, id: string) =>
  `codemux:question-draft:${threadId}:${id}`;
function readDraft(key: string): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(value) && value.every((v) => typeof v === "string")
      ? value
      : [];
  } catch {
    return [];
  }
}

/** Shared presentation; only adapters emitting native async questions reach it. */
export function AsyncQuestionPanel({
  items,
  threadId,
  working,
}: {
  items: AsyncQuestionItem[];
  threadId: string;
  working: boolean;
}) {
  const pending = items.filter(
    (i) =>
      i.resolution.status !== "answered" && i.resolution.status !== "dismissed",
  );
  const dismissed = items.filter((i) => i.resolution.status === "dismissed");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const index = Math.max(
    0,
    pending.findIndex((i) => i.question.id === selectedId),
  );
  const selected = pending[index];
  const [error, setError] = useState<string | null>(null);
  const act = useCallback(
    async (id: string, action: QuestionAction) => {
      setError(null);
      try {
        const resolution = await agentChatAnswerQuestion(threadId, id, action);
        useAgentChatStore.getState().applyEvent(threadId, {
          type: "question_resolved",
          thread_id: threadId,
          question_id: id,
          resolution,
        });
        if (resolution.status === "answered") {
          try {
            localStorage.removeItem(draftKey(threadId, id));
          } catch {
            /* storage may be unavailable */
          }
        }
        if (resolution.status === "failed") throw new Error(resolution.message);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        throw cause;
      }
    },
    [threadId],
  );
  if (!selected && dismissed.length === 0) return null;
  return (
    <section aria-label="Agent questions" className="pb-2">
      <div className={CHAT_COLUMN_OUTER}>
        <div className={CHAT_COLUMN_INNER}>
          {selected && (
            <div className="flex items-center justify-between gap-3 px-3 pb-2 text-xs">
              <div className="flex min-w-0 items-center gap-2">
                <MessageCircleQuestion className="size-3.5 shrink-0 text-primary" />
                <span className="font-medium">
                  {pending.length > 1
                    ? `${pending.length} questions pending`
                    : "Question for you"}
                </span>
                <span className="text-muted-foreground">
                  {working ? "Work is continuing" : "Answer to continue"}
                </span>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                {pending.length > 1 && (
                  <>
                    <button
                      type="button"
                      aria-label="Previous question set"
                      disabled={index === 0}
                      className="rounded p-1 hover:bg-muted disabled:opacity-30"
                      onClick={() =>
                        setSelectedId(pending[index - 1].question.id)
                      }
                    >
                      <ChevronLeft className="size-3.5" />
                    </button>
                    <span className="tabular-nums text-muted-foreground">
                      {index + 1}/{pending.length}
                    </span>
                    <button
                      type="button"
                      aria-label="Next question set"
                      disabled={index === pending.length - 1}
                      className="rounded p-1 hover:bg-muted disabled:opacity-30"
                      onClick={() =>
                        setSelectedId(pending[index + 1].question.id)
                      }
                    >
                      <ChevronRight className="size-3.5" />
                    </button>
                  </>
                )}
                {(selected.resolution.status === "pending" ||
                  selected.resolution.status === "failed") && (
                  <button
                    type="button"
                    aria-label="Dismiss question"
                    className="rounded p-1 text-muted-foreground hover:bg-muted"
                    onClick={() =>
                      void act(selected.question.id, {
                        action: "dismiss",
                      }).catch(() => {})
                    }
                  >
                    <X className="size-3.5" />
                  </button>
                )}
              </div>
            </div>
          )}
          {error && (
            <p role="alert" className="px-3 pb-2 text-xs text-destructive">
              {error}
            </p>
          )}
        </div>
      </div>
      {selected && (
        <QuestionCard
          key={`${selected.question.id}:${selected.resolution.status}`}
          item={selected}
          threadId={threadId}
          act={act}
        />
      )}
      {dismissed.length > 0 && (
        <div className={CHAT_COLUMN_OUTER}>
          <details
            className={cn(CHAT_COLUMN_INNER, "text-xs text-muted-foreground")}
          >
            <summary className="cursor-pointer px-3 py-1">
              Dismissed questions ({dismissed.length})
            </summary>
            {dismissed.map((item) => (
              <button
                key={item.question.id}
                type="button"
                className="block px-3 py-2 text-left hover:text-foreground"
                onClick={() =>
                  void act(item.question.id, { action: "reopen" }).catch(
                    () => {},
                  )
                }
              >
                Reopen: {item.question.questions[0]?.title}
              </button>
            ))}
          </details>
        </div>
      )}
    </section>
  );
}

function QuestionCard({
  item,
  threadId,
  act,
}: {
  item: AsyncQuestionItem;
  threadId: string;
  act: (id: string, action: QuestionAction) => Promise<void>;
}) {
  const key = draftKey(threadId, item.question.id);
  const save = useCallback(
    (answers: string[]) => {
      try {
        localStorage.setItem(key, JSON.stringify(answers));
      } catch {
        /* Form remains usable without storage. */
      }
    },
    [key],
  );
  const questions = useMemo<Question[]>(
    () =>
      item.question.questions.map((q) => ({
        question: q.title,
        header: "Clarification",
        allowOther: true,
        multiSelect: false,
        options: q.options.map((label) => ({
          label,
          description: "",
          preview: null,
        })),
      })),
    [item.question.questions],
  );
  const [initial] = useState(() =>
    "answers" in item.resolution ? item.resolution.answers : readDraft(key),
  );
  const resolution = item.resolution;
  if (resolution.status === "submitting" || resolution.status === "unknown") {
    return (
      <div className={CHAT_COLUMN_OUTER}>
        <div className={CHAT_COLUMN_INNER}>
          <div className="rounded-2xl border border-border bg-muted/30 px-4 py-3 text-xs">
            <p role="status">
              {resolution.status === "submitting"
                ? "Submitting answer…"
                : resolution.message}
            </p>
            {resolution.status === "unknown" && (
              <p className="mt-1 text-muted-foreground">
                Sending again may duplicate your answer.
              </p>
            )}
            <button
              className="mt-2 underline underline-offset-4"
              onClick={() =>
                void act(item.question.id, { action: "reconcile" }).catch(
                  () => {},
                )
              }
            >
              Check delivery
            </button>
            {resolution.status === "unknown" && (
              <button
                className="ml-4 mt-2 underline underline-offset-4"
                onClick={() =>
                  void act(item.question.id, {
                    action: "answer",
                    answers: resolution.answers,
                    submission_id: crypto.randomUUID(),
                    retry_unknown: true,
                  }).catch(() => {})
                }
              >
                Send again
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }
  return (
    <QuestionForm
      idPrefix={item.question.id}
      questions={questions}
      globalShortcuts={false}
      initialAnswers={initial}
      onAnswersChange={save}
      onSubmit={(answers) =>
        act(item.question.id, {
          action: "answer",
          answers,
          submission_id: crypto.randomUUID(),
        })
      }
    />
  );
}
