import {
  MessageCircleQuestion,
  X,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { AsyncQuestionItem } from "@/lib/agent-chat/types";
import { toast } from "@/lib/toast";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { agentChatAnswerQuestion, type QuestionAction } from "@/tauri/commands";
import { QuestionForm, type Question } from "./QuestionForm";
import { CHAT_COLUMN_INNER, CHAT_COLUMN_OUTER } from "./chat-column";
import { COMPOSER_OVERLAY_CARD } from "./composer-overlay";
import { cn } from "@/lib/utils";
import { randomUUID } from "@/lib/uuid";

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
        await resolveQuestion(threadId, id, action);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        throw cause;
      }
    },
    [threadId],
  );
  // Dismissed questions live in the transcript, where each one can be
  // reopened; the panel above the composer is only for open questions.
  if (!selected) return null;
  return (
    <section aria-label="Agent questions" className="pb-2">
      <div className={CHAT_COLUMN_OUTER}>
        <div className={cn(CHAT_COLUMN_INNER, COMPOSER_OVERLAY_CARD)}>
          {selected && (
            <div className="flex items-center justify-between gap-3 px-3 pb-2 text-label">
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
                      className="rounded-sm p-1 hover:bg-muted disabled:opacity-30"
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
                      className="rounded-sm p-1 hover:bg-muted disabled:opacity-30"
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
                    className="rounded-sm p-1 text-muted-foreground hover:bg-muted"
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
            <p role="alert" className="px-3 pb-2 text-label text-destructive">
              {error}
            </p>
          )}
        </div>
      </div>
      <QuestionCard
        key={`${selected.question.id}:${selected.resolution.status}`}
        item={selected}
        threadId={threadId}
        act={act}
      />
    </section>
  );
}

/** Send a question action and fold the result into the thread. Throws when
 *  the command fails or the provider reports the answer failed. */
async function resolveQuestion(
  threadId: string,
  id: string,
  action: QuestionAction,
): Promise<void> {
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
}

/** The thread a transcript belongs to, for rows that act on it. `null` for
 *  a read-only transcript, which must not offer actions. */
export const AsyncQuestionThreadContext = createContext<string | null>(null);

/**
 * A question's record in the transcript. A dismissed question can be
 * reopened from here, which puts it back above the composer.
 */
export function AsyncQuestionRecord({ item }: { item: AsyncQuestionItem }) {
  const threadId = useContext(AsyncQuestionThreadContext);
  const [reopening, setReopening] = useState(false);
  const status = item.resolution.status;
  const reopen = () => {
    if (!threadId || reopening) return;
    setReopening(true);
    resolveQuestion(threadId, item.question.id, { action: "reopen" })
      .catch((cause: unknown) => {
        toast.error("Couldn't reopen the question", {
          description: cause instanceof Error ? cause.message : String(cause),
        });
      })
      .finally(() => setReopening(false));
  };
  return (
    <div className="space-y-1 py-1 text-body">
      {item.question.text && (
        <p className="text-muted-foreground">{item.question.text}</p>
      )}
      {item.question.questions
        .filter((question) => !item.question.text.includes(question.title))
        .map((question, index) => (
          <p key={index}>{question.title}</p>
        ))}
      <p className="flex items-center gap-2 text-label text-muted-foreground">
        {status === "answered"
          ? "Answered"
          : status === "dismissed"
            ? "Dismissed"
            : "Answer above the composer · work can continue"}
        {status === "dismissed" && threadId && (
          <button
            type="button"
            disabled={reopening}
            onClick={reopen}
            className="rounded-sm px-1 font-medium text-foreground/80 underline-offset-4 transition-colors duration-100 hover:text-foreground hover:underline disabled:opacity-50"
          >
            {reopening ? "Reopening…" : "Reopen"}
          </button>
        )}
      </p>
    </div>
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
        // The provider sends no header of its own.
        header: "Question",
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
        <div className={cn(CHAT_COLUMN_INNER, COMPOSER_OVERLAY_CARD)}>
          <div className="rounded-lg border border-border bg-muted/30 px-4 py-3 text-label">
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
                    submission_id: randomUUID(),
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
          submission_id: randomUUID(),
        })
      }
    />
  );
}
