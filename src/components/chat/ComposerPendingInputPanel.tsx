import { useMemo } from "react";
import type { PermissionRequestItem } from "@/lib/agent-chat/types";
import { QuestionForm, type Question } from "./QuestionForm";

export interface AskUserQuestionOutput {
  questions: unknown;
  answers: Record<string, string>;
}

/** Legacy callback wrapper: preserve each provider's existing answer shape. */
export function ComposerPendingInputPanel({
  item,
  onSubmit,
}: {
  item: PermissionRequestItem;
  onSubmit: (output: AskUserQuestionOutput) => void | Promise<void>;
}) {
  const questions = useMemo(
    () => extractQuestions(item.payload),
    [item.payload],
  );
  return (
    <QuestionForm
      questions={questions}
      active={item.resolution.state === "pending"}
      onSubmit={(answers) =>
        onSubmit({
          questions: isRecord(item.payload)
            ? (item.payload.questions ?? [])
            : [],
          answers: Object.fromEntries(
            questions.map((q, i) => [q.question, answers[i]]),
          ),
        })
      }
    />
  );
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
