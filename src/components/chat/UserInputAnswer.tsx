import { memo } from "react";

import type { PermissionRequestItem } from "@/lib/agent-chat/types";

/**
 * Right-aligned reply bubble that echoes the user's answer to an
 * AskUserQuestion prompt (`request_kind: "user-input"`) once it has
 * resolved. The interactive picker lives above the composer
 * (ComposerPendingInputPanel); after the user submits, that panel is
 * gone, so without this the transcript only showed a muted "Answered"
 * marker and it never looked like the user replied.
 *
 * The answer is already stored on the resolved request — the sidecar
 * relays the picker's `AskUserQuestionOutput` back as the decision's
 * `updated_input`, whose `answers` map is keyed by question text with
 * the chosen option label(s) as the value (comma-joined for
 * multiSelect). We read it back here instead of re-deriving anything.
 *
 * Styling deliberately mirrors `UserMessage` (same card fill, border,
 * and the asymmetric `14px 14px 5px 14px` radius that tucks toward the
 * composer) so a submitted answer reads as a genuine user reply. All
 * colors are theme tokens.
 */
export const UserInputAnswer = memo(function UserInputAnswer({
  item,
}: {
  item: PermissionRequestItem;
}) {
  const lines = extractAnswerLines(item);

  // Resolved without a readable answer (e.g. the prompt was cancelled,
  // or a future decision shape we don't parse) — fall back to the
  // original muted marker so the row is never blank.
  if (lines.length === 0) {
    return <div className="py-0.5 text-xs text-muted-foreground">Answered</div>;
  }

  // With a single question the value speaks for itself; with several,
  // label each answer by its question header so the reply stays legible.
  const showHeaders = lines.length > 1;

  return (
    <div className="flex justify-end">
      <div className="flex max-w-[82%] flex-col items-end gap-1">
        <div className="flex flex-col gap-2 rounded-[14px_14px_5px_14px] border border-border/60 bg-card px-[15px] py-[11px] text-sm leading-relaxed text-foreground">
          {lines.map((line, i) => (
            <div key={i} className="flex flex-col gap-0.5">
              {showHeaders && line.header ? (
                <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                  {line.header}
                </div>
              ) : null}
              <div className="select-text whitespace-pre-wrap break-words">
                {line.value}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
});

interface AnswerLine {
  /** Short question header (the chip label), when we can recover it from
   *  the original prompt payload. `null` falls back to no label. */
  header: string | null;
  /** The chosen option label(s) — the user's actual answer. */
  value: string;
}

/** Pull the human-readable answer(s) off a resolved user-input request.
 *  Returns `[]` for any non-resolved / non-`allow` / unparseable shape so
 *  the caller can degrade to the plain marker. */
function extractAnswerLines(item: PermissionRequestItem): AnswerLine[] {
  if (item.resolution.state !== "resolved") return [];
  const { decision } = item.resolution;
  if (decision.decision !== "allow") return [];

  const answers = readAnswers(decision.updated_input);
  if (!answers) return [];

  const headerByQuestion = buildHeaderMap(item.payload);

  return Object.entries(answers)
    .filter(
      ([, value]) => typeof value === "string" && value.trim().length > 0,
    )
    .map(([question, value]) => ({
      header: headerByQuestion.get(question) ?? null,
      value: value as string,
    }));
}

/** The `answers` map off an `AskUserQuestionOutput`, or `null` if the
 *  payload isn't the shape we expect. */
function readAnswers(input: unknown): Record<string, unknown> | null {
  if (!isRecord(input)) return null;
  const answers = input["answers"];
  if (!isRecord(answers)) return null;
  return answers;
}

/** Map question text → its short `header` chip, read from the original
 *  prompt payload so multi-question replies can be labeled. */
function buildHeaderMap(payload: unknown): Map<string, string> {
  const map = new Map<string, string>();
  if (!isRecord(payload)) return map;
  const questions = payload["questions"];
  if (!Array.isArray(questions)) return map;
  for (const q of questions) {
    if (!isRecord(q)) continue;
    const question = q["question"];
    const header = q["header"];
    if (typeof question === "string" && typeof header === "string") {
      map.set(question, header);
    }
  }
  return map;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
