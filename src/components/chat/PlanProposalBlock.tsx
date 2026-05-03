import { useState } from "react";

import { Button } from "@/components/ui/button";
import type { PermissionRequestItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";
import { ToolCallBlock } from "./ToolCallBlock";

interface Props {
  /** Must be `request_kind === "plan"`. The payload is the
   *  `ExitPlanModeInput` the model emitted — a markdown `plan`
   *  string with optional `allowedPrompts`. */
  item: PermissionRequestItem;
  /** Accept & execute: flip permission mode to `default` live and
   *  send a synthetic "Proceed with the plan." turn. */
  onAccept: () => void | Promise<void>;
  /** Reject: parent sends a generic "Please revise the plan." turn. */
  onReject: () => void | Promise<void>;
}

/**
 * Specialized renderer for `request_kind: "plan"`. The generic
 * PermissionRequestBlock falls back to a JSON dump, which loses all
 * structure on a markdown plan. This component renders:
 *
 *   • a muted "Plan proposed" label so the card is visually distinct
 *     from a regular assistant message
 *   • the plan body via `ChatMarkdown` (headings, bullets, code
 *     blocks styled to match assistant prose)
 *   • an optional `allowedPrompts` summary line when the model
 *     declared the Bash prompts it needs
 *   • Accept / Reject buttons — both fire their handler immediately,
 *     matching Cursor / VS Code plan UIs (no follow-up feedback popup)
 *
 * Defensive fallback: if `payload.plan` is not a string (unexpected
 * SDK shape), the block renders the raw payload via ToolCallBlock so
 * nothing is silently dropped.
 */
export function PlanProposalBlock({ item, onAccept, onReject }: Props) {
  const [submitted, setSubmitted] = useState(false);

  if (item.resolution.state !== "pending") {
    // Collapse to a terminal one-liner once resolution has landed,
    // matching PermissionRequestBlock's resolved/responding behavior
    // so the transcript doesn't keep showing interactive controls
    // after the user has acted.
    return (
      <div className="py-0.5 text-xs text-muted-foreground">
        {resolvedLabel(item.resolution.state)}
      </div>
    );
  }

  const plan = extractPlanText(item.payload);
  const allowedPrompts = extractAllowedPrompts(item.payload);

  const submitAccept = async () => {
    if (submitted) return;
    setSubmitted(true);
    try {
      await onAccept();
    } catch {
      // Let the parent surface the error via toast; re-enable the
      // buttons so the user can retry.
      setSubmitted(false);
    }
  };
  const submitReject = async () => {
    if (submitted) return;
    setSubmitted(true);
    try {
      await onReject();
    } catch {
      setSubmitted(false);
    }
  };

  return (
    // `overflow-hidden` clips any stray protrusion (wide code, long
    // unbreakable tokens, list-marker overflow) at the rounded
    // border. Internal scroll surfaces (`pre`, `table`) still get
    // their own `overflow-x-auto` so scrollable content stays
    // interactive.
    <div className="rounded-md bg-muted/30 p-3 space-y-3 overflow-hidden">
      <div className="text-xs text-muted-foreground">Plan proposed</div>

      {plan !== null ? (
        <div className="text-sm leading-relaxed text-foreground break-words">
          <ChatMarkdown>{plan}</ChatMarkdown>
        </div>
      ) : (
        // Unexpected SDK shape — show whatever we got rather than
        // swallow it. Same tier-3 recessed monospace as tool output.
        <ToolCallBlock content={null} text={safeStringify(item.payload)} />
      )}

      {allowedPrompts.length > 0 && (
        <div className="text-[11px] text-muted-foreground/80 space-y-0.5">
          <div className="text-muted-foreground">Will run:</div>
          {allowedPrompts.map((p, i) => (
            <div key={i} className="font-mono">
              {p.tool}: &ldquo;{p.prompt}&rdquo;
            </div>
          ))}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          size="sm"
          // Overlay-button token per the codemux-ui skill: action
          // buttons inside approval surfaces use the neutral
          // foreground/background pair instead of the `--primary`
          // accent, so the card doesn't compete with the chat's tone.
          // Matches AskUserQuestionBlock's Submit and the Send button.
          className="h-7 px-3 text-xs bg-foreground text-background hover:bg-foreground/90"
          onClick={submitAccept}
          disabled={submitted}
        >
          Accept &amp; execute
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7 px-3 text-xs"
          onClick={submitReject}
          disabled={submitted}
        >
          Reject
        </Button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function extractPlanText(payload: unknown): string | null {
  if (!isRecord(payload)) {
    return typeof payload === "string" ? (payload as string) : null;
  }
  const plan = payload["plan"];
  return typeof plan === "string" ? plan : null;
}

interface AllowedPrompt {
  tool: string;
  prompt: string;
}

function extractAllowedPrompts(payload: unknown): AllowedPrompt[] {
  if (!isRecord(payload)) return [];
  const raw = payload["allowedPrompts"];
  if (!Array.isArray(raw)) return [];
  const out: AllowedPrompt[] = [];
  for (const entry of raw) {
    if (!isRecord(entry)) continue;
    const tool = entry["tool"];
    const prompt = entry["prompt"];
    if (typeof tool === "string" && typeof prompt === "string") {
      out.push({ tool, prompt });
    }
  }
  return out;
}

function resolvedLabel(
  state: "pending" | "responding" | "resolved",
): string {
  // `pending` is handled above; this only fires for terminal states.
  switch (state) {
    case "responding":
      return "Submitting decision…";
    case "resolved":
      return "Plan handled";
    case "pending":
      return "";
  }
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function safeStringify(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
