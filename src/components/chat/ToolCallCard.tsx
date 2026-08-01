import { memo, useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Clock,
  Loader2,
  X,
  type LucideIcon,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  buildPermissionUpdate,
  type PermissionScope,
} from "@/lib/agent-chat/permission-rules";
import { isLazyToolResultStub } from "@/lib/agent-chat/lazy-tool-result";
import { hasToolResultImages } from "@/lib/agent-chat/tool-result-images";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import type {
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { ToolCallBlock } from "./ToolCallBlock";
import { ToolCallBody } from "./ToolCallBodies";
import { ToolCallStatus } from "./ToolCallStatus";
import { categoryTint, toolCategory, toolIcon } from "./tool-visuals";

interface Props {
  item: ToolCallItem;
  /** Resolved from the slice by matching `item.approval_request_id`
   *  against the thread's permission requests. `null` when the tool
   *  call is not gated (bypassPermissions mode) or the request event
   *  hasn't landed yet. */
  approval: PermissionRequestItem | null;
  onDecide: (decision: ApprovalDecision) => void;
}

/**
 * Stage 1 merged card — tool call header, result body, and inline
 * approval footer in one row. Replaces the two stacked rows that the
 * prior ToolCallStatus + ToolCallBlock + PermissionRequestBlock
 * arrangement produced.
 *
 * Six visual states driven by `(approval.resolution, item.status)`:
 *
 *   pending_approval  approval=pending           → header + expanded input + Allow/Deny
 *   responding        approval=responding        → header + "Submitting decision…"
 *   expired           approval=failed            → error header + expiry explanation
 *   denied            approval=resolved & deny   → header with strike-through + one-liner
 *   executing         no approval & status=running → header + spinner, body collapsed
 *   success           status=done                → header + check, body collapsed, expandable
 *   error             status=error               → header with muted-red target + X, body expanded
 */
export const ToolCallCard = memo(function ToolCallCard({
  item,
  approval,
  onDecide,
}: Props) {
  const resolution = approval?.resolution;
  const isPendingApproval = resolution?.state === "pending";
  const isResponding = resolution?.state === "responding";
  const isRequestFailed = resolution?.state === "failed";
  const isDenied =
    resolution?.state === "resolved" && resolution.decision.decision !== "allow";
  const isExecuting =
    !isPendingApproval &&
    !isResponding &&
    !isRequestFailed &&
    !isDenied &&
    item.status === "running";
  const isSuccess = item.status === "done";
  const isError = item.status === "error";

  // Default collapsed behavior:
  //  - pending_approval: expanded so user can see what they're approving.
  //  - error: expanded (auto-expand on error).
  //  - Edit/Write diffs: expanded so the diff card reads inline (design D7).
  //  - everything else: collapsed; user can toggle.
  const isDiffTool =
    item.tool_name === "Edit" ||
    item.tool_name === "MultiEdit" ||
    item.tool_name === "Write";
  // A result carrying an image (e.g. a screenshot) is a visual payload
  // meant to be seen — expand it inline like a diff rather than hiding
  // it behind a chevron.
  const hasImages = hasToolResultImages(item.result_content);
  const defaultExpanded = isPendingApproval || isError || isDiffTool || hasImages;
  const [expanded, setExpanded] = useState(defaultExpanded);
  const hasSeenImagesRef = useRef(hasImages);

  // Tool cards usually mount while the call is still running, before
  // `result_content` exists. Open once when a renderable image first arrives;
  // tracking the transition prevents ordinary rerenders from undoing a user's
  // later manual collapse.
  useEffect(() => {
    if (hasImages && !hasSeenImagesRef.current) {
      setExpanded(true);
      hasSeenImagesRef.current = true;
    }
  }, [hasImages]);

  const Icon = toolIcon(item.tool_name);
  const glyph = glyphForState({
    isPendingApproval,
    isResponding,
    isDenied,
    isExecuting,
    isSuccess,
    isError,
  });

  const hasResultBody = hasRenderableContent(item.result_content);
  const inputText = hasRenderableInput(item.input)
    ? safeStringify(item.input)
    : null;
  const canExpand = hasResultBody || inputText !== null;
  const showBody =
    expanded && !isPendingApproval && !isResponding && !isDenied;

  return (
    <div className="overflow-hidden rounded-[10px] border border-border/60 bg-muted/40">
      {/* Header row: tinted icon chip · mono command · status glyph ·
          chevron. `min-w-0 truncate` on the label lets long commands
          ellipsize rather than push the trailing glyphs off-screen. */}
      <div className="flex items-center gap-2.5 px-3 py-2.5 min-w-0">
        <span
          className={cn(
            "flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-md",
            categoryTint(toolCategory(item.tool_name)),
            isDenied && "opacity-50",
          )}
        >
          <Icon className="h-3 w-3" strokeWidth={1.5} aria-hidden />
        </span>
        <div
          className={cn(
            "min-w-0 flex-1 truncate",
            isDenied && "line-through text-muted-foreground/60",
          )}
        >
          <ToolCallStatus item={item} />
        </div>
        {glyph && (
          <glyph.Icon
            className={cn("h-3.5 w-3.5 shrink-0", glyph.className)}
            aria-hidden
          />
        )}
        {canExpand && !isPendingApproval && !isResponding && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="shrink-0 text-muted-foreground/60 hover:text-foreground"
            aria-label={expanded ? "Collapse" : "Expand"}
          >
            {expanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
          </button>
        )}
      </div>

      {/* Approval footer (pending). Keyed on the approval request id so
          a fresh approval (different request_id) remounts the footer
          and clears the deny textarea / dropdown state — otherwise
          stale text from a prior denial leaks into the next prompt. */}
      {isPendingApproval && approval && (
        <ApprovalFooter
          key={approval.request_id}
          inputText={inputText}
          onDecide={onDecide}
          toolName={item.tool_name}
        />
      )}

      {/* In-flight decision marker */}
      {isResponding && (
        <div className="border-t border-border/60 px-3 py-2 text-xs text-muted-foreground/70">
          Submitting decision…
        </div>
      )}

      {isRequestFailed && resolution?.state === "failed" && (
        <div className="border-t border-border/60 px-3 py-2 text-xs text-muted-foreground">
          {resolution.message}
        </div>
      )}

      {/* Denied terminal state */}
      {isDenied && resolution?.state === "resolved" && (
        <div className="border-t border-border/60 px-3 py-2 text-xs text-muted-foreground">
          {denialLabel(resolution.decision)}
        </div>
      )}

      {/* Result body when expanded — known tools get a polished
          renderer, unknown tools fall back to the raw JSON dump. */}
      {showBody && (
        <div className="border-t border-border/60 px-3 py-2.5">
          <ToolCallBody item={item} />
        </div>
      )}
    </div>
  );
});

// ---------------------------------------------------------------------------
// Approval footer
// ---------------------------------------------------------------------------

interface ApprovalFooterProps {
  inputText: string | null;
  toolName: string;
  onDecide: (decision: ApprovalDecision) => void;
}

function ApprovalFooter({ inputText, onDecide, toolName }: ApprovalFooterProps) {
  const [denying, setDenying] = useState(false);
  const [reason, setReason] = useState("");
  // Synchronous in-flight guard. The parent flips
  // `approval.resolution` to `responding` after the IPC round-trips,
  // but rapid double-clicks can fire `handleAllow` (or `confirmDeny`)
  // in the same tick before React re-renders. Once we've dispatched
  // a decision for this approval, all further clicks are dropped —
  // the footer is about to unmount when `responding` lands.
  const dispatchedRef = useRef(false);

  const handleAllow = (scope: PermissionScope) => {
    if (dispatchedRef.current) return;
    // Stage 5 omits ruleContent — every "Allow always" matches any
    // input for the tool. Stage 7 will add command-specific rules.
    const updatedPermissions = buildPermissionUpdate(scope, { toolName });
    if (scope !== "once" && !updatedPermissions) {
      // Defensive: helper returned undefined for a persistent scope
      // (currently only possible if `PermissionScope` gains a new
      // member that the helper hasn't been taught). Drop to a
      // one-shot allow rather than silently writing nothing or
      // accidentally targeting `userSettings`.
      onDecide({ decision: "allow" });
      dispatchedRef.current = true;
      return;
    }
    dispatchedRef.current = true;
    onDecide({
      decision: "allow",
      ...(updatedPermissions ? { updated_permissions: updatedPermissions } : {}),
    });
    // Toast wording is action-oriented (not past-tense) because the
    // SDK persists the rule asynchronously and the sidecar does not
    // currently surface a write-failed signal. The settings-file
    // path is shown so the user can verify.
    if (scope === "project") {
      toast.success(`Allowing ${toolName} for this project`, {
        description: "Rule saved to .claude/settings.local.json",
      });
    } else if (scope === "user") {
      toast.success(`Allowing ${toolName} for all projects`, {
        description: "Rule saved to ~/.claude/settings.json",
      });
    }
  };

  const confirmDeny = () => {
    if (dispatchedRef.current) return;
    dispatchedRef.current = true;
    onDecide({ decision: "deny", message: reason || "User denied" });
  };

  return (
    <div className="border-t border-border/60 p-3 space-y-2">
      {inputText !== null && (
        <ToolCallBlock content={null} text={inputText} />
      )}
      {!denying ? (
        // Approval hierarchy:
        //   • Allow         → primary affirmative (overlay-button token,
        //                     same pattern as PlanProposalBlock's
        //                     "Accept & execute" and PermissionRequest-
        //                     Block's Allow).
        //   • Allow always  → outline (still affirmative, but persistent
        //                     scope; the dropdown caret signals the
        //                     extra choice).
        //   • Deny          → ghost-muted (passive; only takes focus if
        //                     the user actively wants to refuse).
        // No accent colour anywhere — the chat-ui skill keeps the
        // conversation neutral.
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="sm"
            className="h-7 px-3 text-xs bg-foreground text-background hover:bg-foreground/90"
            onClick={() => handleAllow("once")}
          >
            Allow
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 px-3 text-xs"
              >
                Allow always
                <ChevronDown className="ml-1 h-3 w-3" aria-hidden />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="text-xs">
              <DropdownMenuItem
                onSelect={() => handleAllow("project")}
                className="text-xs gap-3"
              >
                <span>For this project</span>
                <span className="ml-auto text-[10px] text-muted-foreground">
                  .claude/settings.local.json
                </span>
              </DropdownMenuItem>
              <DropdownMenuItem
                onSelect={() => handleAllow("user")}
                className="text-xs gap-3"
              >
                <span>For all projects</span>
                <span className="ml-auto text-[10px] text-muted-foreground">
                  ~/.claude/settings.json
                </span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
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
          {/* Destructive intent — the sole sanctioned use of --danger
              in the conversation (chat-ui skill). Cancel drops to
              ghost so Confirm-deny reads as the active choice. */}
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface StatusGlyph {
  Icon: LucideIcon;
  className: string;
}

/** Trailing status glyph + its tint. Green check on success, red ✗ on
 *  error, ember spinner while executing — neutral for the transient
 *  approval states. */
function glyphForState(states: {
  isPendingApproval: boolean;
  isResponding: boolean;
  isDenied: boolean;
  isExecuting: boolean;
  isSuccess: boolean;
  isError: boolean;
}): StatusGlyph | null {
  if (states.isPendingApproval) return { Icon: Clock, className: "text-muted-foreground" };
  if (states.isResponding)
    return { Icon: Loader2, className: "animate-spin text-muted-foreground" };
  if (states.isExecuting)
    return { Icon: Loader2, className: "animate-spin text-accent-ember" };
  if (states.isSuccess) return { Icon: Check, className: "text-status-open" };
  if (states.isError) return { Icon: X, className: "text-status-attention" };
  if (states.isDenied) return { Icon: X, className: "text-muted-foreground" };
  return null;
}

function denialLabel(decision: ApprovalDecision): string {
  switch (decision.decision) {
    case "deny":
      return `Denied${decision.message ? `: ${decision.message}` : ""}`;
    case "cancel":
      return "Cancelled";
    // `allow` / `allow_for_session` would never land here because
    // the caller checks for `decision !== "allow"` before rendering
    // this branch — but the switch is exhaustive so TypeScript
    // doesn't narrow against it.
    case "allow":
    case "allow_for_session":
      return "Allowed";
  }
}

function hasRenderableContent(content: unknown): boolean {
  if (content == null) return false;
  // A lazily-stubbed body renders (preview + a fetch affordance), so the
  // chevron must stay available.
  if (isLazyToolResultStub(content)) return true;
  if (typeof content === "string") return content.length > 0;
  if (Array.isArray(content)) return content.length > 0;
  return true;
}

function hasRenderableInput(input: unknown): boolean {
  if (input == null) return false;
  if (typeof input === "object") {
    return Object.keys(input as Record<string, unknown>).length > 0;
  }
  return true;
}

function safeStringify(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
