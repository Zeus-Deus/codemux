import { ChevronDown, ChevronUp } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { AssistantMessage } from "./AssistantMessage";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { PlanProposalBlock } from "./PlanProposalBlock";
import { ToolCallCard } from "./ToolCallCard";
import { UserMessage } from "./UserMessage";

/** A contiguous stretch of this many tool-call status lines gets
 *  collapsed by default. Anything shorter renders inline as before. */
const RUN_COLLAPSE_THRESHOLD = 6;
/** When a run is collapsed, keep this many most-recent tool calls
 *  visible — "a handful of the most recent tool calls happening." */
const RUN_TAIL_VISIBLE = 4;

interface Props {
  messages: ChatViewItem[];
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  /** Plan-accept: parent flips the live session to `default` mode
   *  and sends a "Proceed with the plan." synthetic turn. The
   *  request_id is the id of the plan `PermissionRequestItem` the
   *  user clicked — the parent uses it to collapse the card locally
   *  (no sidecar `request-resolved` will ever arrive). */
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  /** Plan-reject: parent sends a generic "Please revise the plan."
   *  turn. No feedback is collected from the user — matching the
   *  Cursor / VS Code plan UIs. */
  onRejectPlan: (requestId: string) => void | Promise<void>;
}

export function MessageList({
  messages,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: Props) {
  // Sort by seq so order is a property of the data, not of React
  // reconciliation or store-update timing. Stable for equal seq via
  // id tiebreak (should never happen in practice — seq is unique
  // per-thread — but cheap insurance for future callers).
  const ordered = useMemo(() => {
    const copy = messages.slice();
    copy.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
    return copy;
  }, [messages]);

  // Index permission requests by request_id so the tool_call branch can
  // look up the matching approval for its inline footer in O(1).
  const requestsById = useMemo(() => {
    const map = new Map<string, PermissionRequestItem>();
    for (const m of ordered) {
      if (m.kind === "permission_request") {
        map.set(m.request_id, m);
      }
    }
    return map;
  }, [ordered]);

  // Requests merged into a ToolCallCard via approval_request_id should
  // not also render as a standalone row — the card already owns their
  // approval footer. Build the set once per render.
  const mergedRequestIds = useMemo(() => {
    const set = new Set<string>();
    for (const m of ordered) {
      if (m.kind === "tool_call" && m.approval_request_id) {
        set.add(m.approval_request_id);
      }
    }
    return set;
  }, [ordered]);

  // Group consecutive tool-call rows into runs so long bursts (e.g. a
  // burst of Read / Ran calls at the start of a plan) collapse to a
  // "Show N earlier tool calls" toggle instead of cluttering the
  // transcript. Non-tool items break a run. Tool calls carrying an
  // active approval footer stay out of runs — the approval needs to
  // remain interactable without the user hunting for it.
  const slots = useMemo<RenderSlot[]>(() => {
    const out: RenderSlot[] = [];
    let run: ToolCallItem[] = [];
    const flush = () => {
      if (run.length === 0) return;
      if (run.length >= RUN_COLLAPSE_THRESHOLD) {
        out.push({ kind: "toolRun", items: run });
      } else {
        for (const t of run) out.push({ kind: "item", item: t });
      }
      run = [];
    };
    for (const item of ordered) {
      if (
        item.kind === "tool_call" &&
        item.approval_request_id == null
      ) {
        run.push(item);
      } else {
        flush();
        out.push({ kind: "item", item });
      }
    }
    flush();
    return out;
  }, [ordered]);

  const renderItem = (item: ChatViewItem): ReactNode => (
    <MessageRow
      key={item.id}
      item={item}
      requestsById={requestsById}
      mergedRequestIds={mergedRequestIds}
      onRespondToRequest={onRespondToRequest}
      onAcceptPlan={onAcceptPlan}
      onRejectPlan={onRejectPlan}
    />
  );

  return (
    <div className="flex flex-col gap-3">
      {slots.map((slot) => {
        if (slot.kind === "item") return renderItem(slot.item);
        return (
          <ToolRunCollapse
            // Key on the first item's id so React preserves expand
            // state across re-renders even when new tool calls arrive
            // at the tail of a streaming run.
            key={`run:${slot.items[0].id}`}
            items={slot.items}
            renderItem={renderItem}
          />
        );
      })}
    </div>
  );
}

type RenderSlot =
  | { kind: "item"; item: ChatViewItem }
  | { kind: "toolRun"; items: ToolCallItem[] };

/**
 * Collapses a stretch of ≥ RUN_COLLAPSE_THRESHOLD consecutive tool
 * calls into a "Show N earlier tool calls" toggle + the last
 * RUN_TAIL_VISIBLE rows. Click to expand; click again to collapse.
 *
 * Uses `display: contents` on the wrapper so the inner rows remain
 * direct flex children of the parent transcript and inherit its
 * `gap-3` vertical rhythm without introducing a nested box.
 */
function ToolRunCollapse({
  items,
  renderItem,
}: {
  items: ToolCallItem[];
  renderItem: (item: ChatViewItem) => ReactNode;
}) {
  const [expanded, setExpanded] = useState(false);
  const hiddenCount = items.length - RUN_TAIL_VISIBLE;
  const visible = expanded ? items : items.slice(-RUN_TAIL_VISIBLE);
  const pluralSuffix = hiddenCount === 1 ? "" : "s";

  return (
    <div className="contents">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex items-center gap-1.5 py-0.5 text-xs text-muted-foreground/80 hover:text-foreground transition-colors"
      >
        {expanded ? (
          <ChevronUp className="h-3 w-3" aria-hidden />
        ) : (
          <ChevronDown className="h-3 w-3" aria-hidden />
        )}
        <span>
          {expanded
            ? `Hide ${hiddenCount} earlier tool call${pluralSuffix}`
            : `Show ${hiddenCount} earlier tool call${pluralSuffix}`}
        </span>
      </button>
      {visible.map((item) => renderItem(item))}
    </div>
  );
}

function MessageRow({
  item,
  requestsById,
  mergedRequestIds,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: {
  item: ChatViewItem;
  requestsById: Map<string, PermissionRequestItem>;
  mergedRequestIds: Set<string>;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
}) {
  switch (item.kind) {
    case "user_message":
      return <UserMessage item={item} />;
    case "assistant_message":
      return <AssistantMessage item={item} />;
    case "tool_call": {
      const approval =
        item.approval_request_id != null
          ? requestsById.get(item.approval_request_id) ?? null
          : null;
      return (
        <ToolCallCard
          item={item}
          approval={approval}
          onDecide={(decision) => {
            if (item.approval_request_id) {
              onRespondToRequest(item.approval_request_id, decision);
            }
          }}
        />
      );
    }
    case "permission_request": {
      // Requests already consumed by a ToolCallCard render there.
      if (mergedRequestIds.has(item.request_id)) return null;
      // Stage 2 dispatch by request_kind: plan / user-input get
      // specialized renderers; everything else stays on the generic
      // PermissionRequestBlock fallback so future kinds (MCP server,
      // directory approval, …) keep working without a new branch.
      switch (item.request_kind) {
        case "plan":
          return (
            <PlanProposalBlock
              item={item}
              onAccept={() => onAcceptPlan(item.request_id)}
              onReject={() => onRejectPlan(item.request_id)}
            />
          );
        case "user-input": {
          // The interactive panel for user-input prompts lives attached
          // to the composer (see `ComposerPendingInputPanel` mounted in
          // AgentChatPane), not inline. Render a tiny status marker in
          // the transcript so the turn's thread-of-events stays
          // readable and the resolution state still surfaces.
          const label =
            item.resolution.state === "pending"
              ? "Input requested — answer above the composer."
              : item.resolution.state === "responding"
                ? "Submitting answers…"
                : "Answered";
          return (
            <div className="py-0.5 text-xs text-muted-foreground">{label}</div>
          );
        }
        default:
          return (
            <PermissionRequestBlock
              item={item}
              onDecide={(decision) =>
                onRespondToRequest(item.request_id, decision)
              }
            />
          );
      }
    }
    case "turn_ended":
      if (item.status.kind !== "error") return null;
      return (
        <div className="py-0.5 text-xs text-muted-foreground">
          Turn ended: {item.status.subtype}
          {item.status.message ? ` — ${item.status.message}` : ""}
        </div>
      );
  }
}
