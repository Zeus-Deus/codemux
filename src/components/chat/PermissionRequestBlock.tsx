import { memo, useState } from "react";

import { Button } from "@/components/ui/button";
import type { PermissionRequestItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { ToolCallBlock } from "./ToolCallBlock";

interface Props {
  item: PermissionRequestItem;
  onDecide: (decision: ApprovalDecision) => void;
}

/**
 * Minimal inline approval prompt (Step 2). No modal, no overlay.
 * Three buttons: Allow / Deny / Allow-for-session. Deny reveals a
 * reason textarea before confirming. Once resolved the block collapses
 * to a one-line status.
 */
export const PermissionRequestBlock = memo(function PermissionRequestBlock({
  item,
  onDecide,
}: Props) {
  const [denying, setDenying] = useState(false);
  const [reason, setReason] = useState("");

  if (item.resolution.state === "resolved") {
    return (
      <div className="py-0.5 text-xs text-muted-foreground">
        {resolvedLabel(item.resolution.decision)}
      </div>
    );
  }

  if (item.resolution.state === "responding") {
    return (
      <div className="py-0.5 text-xs text-muted-foreground">Pending…</div>
    );
  }

  if (item.resolution.state === "failed") {
    return (
      <div className="py-0.5 text-xs text-muted-foreground">
        {item.resolution.message}
      </div>
    );
  }

  const toolName = readToolName(item.payload);
  const toolInput = readToolInput(item.payload);
  const inputText =
    toolInput != null ? JSON.stringify(toolInput, null, 2) : null;

  const allow = () => onDecide({ decision: "allow" });
  const allowSession = () => onDecide({ decision: "allow_for_session" });
  const confirmDeny = () => {
    onDecide({ decision: "deny", message: reason || "User denied" });
  };

  return (
    <div className="rounded-[10px] border border-border/60 bg-muted/40 p-3 space-y-2">
      <div className="text-xs text-muted-foreground">
        Approval requested{toolName ? `: ${toolName}` : ""}
      </div>
      {inputText && (
        <ToolCallBlock content={null} text={inputText} />
      )}
      {!denying ? (
        // Allow is the primary affirmative path, so promote it to the
        // overlay-button token (bg-foreground text-background) — same
        // pattern PlanProposalBlock's "Accept & execute" uses. Keeps
        // the approval hierarchy obvious without reaching for accent
        // colour inside the conversation.
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="sm"
            className="h-7 px-3 text-xs bg-foreground text-background hover:bg-foreground/90"
            onClick={allow}
          >
            Allow
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={allowSession}
          >
            Allow for session
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-3 text-xs text-muted-foreground hover:text-foreground"
            onClick={() => setDenying(true)}
          >
            Deny
          </Button>
        </div>
      ) : (
        <div className="space-y-2">
          <textarea
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            placeholder="Reason (optional)"
            className="w-full resize-none rounded-md bg-background px-2 py-1.5 text-xs text-foreground outline-none ring-1 ring-border focus:ring-muted-foreground/60"
            rows={2}
          />
          {/* Confirm-deny is the sole sanctioned use of --danger in
              the conversation per the chat-ui skill (destructive
              confirmations via variant=destructive). Cancel drops to
              ghost so the primary intent reads at a glance. */}
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="destructive"
              size="sm"
              className="h-7 px-3 text-xs"
              onClick={confirmDeny}
            >
              Confirm deny
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-3 text-xs text-muted-foreground hover:text-foreground"
              onClick={() => {
                setDenying(false);
                setReason("");
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  );
});

function resolvedLabel(decision: ApprovalDecision): string {
  switch (decision.decision) {
    case "allow":
      return "Allowed";
    case "allow_for_session":
      return "Allowed for session";
    case "deny":
      return `Denied${decision.message ? `: ${decision.message}` : ""}`;
    case "cancel":
      return "Cancelled";
  }
}

function readToolName(payload: unknown): string | null {
  if (!isRecord(payload)) return null;
  const name = payload["tool_name"];
  return typeof name === "string" ? name : null;
}

function readToolInput(payload: unknown): unknown {
  if (!isRecord(payload)) return null;
  if ("tool_input" in payload) return payload["tool_input"];
  if ("input" in payload) return payload["input"];
  return null;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
