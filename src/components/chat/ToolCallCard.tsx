import { HermesPermissionOptions, isHermesPermission, hermesPermissionAllowed } from "./HermesPermissionOptions";
import { memo, useEffect, useMemo, useRef, useState } from "react";
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
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  buildPermissionUpdate,
  formatPermissionRule,
  suggestPermissionRule,
  type PermissionRuleSpec,
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

import { useChatProvider } from "./chat-provider-context";
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
    resolution?.state === "resolved" && resolution.decision.decision !== "allow" && resolution.decision.decision !== "allow_for_session" && !(resolution.decision.decision === "provider_option" && hermesPermissionAllowed(approval?.payload, resolution.decision.option_id));
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
    <div
      data-approval-pending={isPendingApproval || undefined}
      className={cn(
        "overflow-hidden rounded-lg border bg-muted/40",
        // A card waiting on the user must not look like a finished one.
        isPendingApproval
          ? "border-status-working/40 ring-1 ring-status-working/40"
          : "border-border/60",
      )}
    >
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
          <Icon className="size-3" aria-hidden />
        </span>
        <div
          className={cn(
            "min-w-0 flex-1 truncate",
            isDenied && "line-through text-muted-foreground/60",
          )}
        >
          <ToolCallStatus item={item} />
          {item.status === "unconfirmed" && <span className="ml-2 text-label text-muted-foreground">Outcome unconfirmed · Hermes did not report completion</span>}
        </div>
        {glyph && (
          <glyph.Icon
            className={cn("size-3.5 shrink-0", glyph.className)}
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
              <ChevronDown className="size-3" />
            ) : (
              <ChevronRight className="size-3" />
            )}
          </button>
        )}
      </div>

      {/* Approval footer (pending). Keyed on the approval request id so
          a fresh approval (different request_id) remounts the footer
          and clears the deny textarea / dropdown state — otherwise
          stale text from a prior denial leaks into the next prompt. */}
      {isPendingApproval && approval && (isHermesPermission(approval.payload) ? <HermesPermissionOptions key={approval.request_id} payload={approval.payload} onDecide={onDecide} /> : (
        <ApprovalFooter
          key={approval.request_id}
          requestId={approval.request_id}
          inputText={inputText}
          onDecide={onDecide}
          toolName={item.tool_name}
          toolInput={item.input}
        />
      ))}

      {/* In-flight decision marker */}
      {isResponding && (
        <div className="border-t border-border/60 px-3 py-2 text-label text-muted-foreground/70">
          Submitting decision…
        </div>
      )}

      {isRequestFailed && resolution?.state === "failed" && (
        <div className="select-text border-t border-border/60 px-3 py-2 text-label text-muted-foreground">
          {resolution.message}
        </div>
      )}

      {/* Denied terminal state */}
      {isDenied && resolution?.state === "resolved" && (
        <div className="border-t border-border/60 px-3 py-2 text-label text-muted-foreground">
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
  requestId: string;
  inputText: string | null;
  toolName: string;
  toolInput: unknown;
  onDecide: (decision: ApprovalDecision) => void;
}

/** Request ids whose Allow button already took focus once. A virtualized
 *  row remounts when it scrolls back into view and must not grab focus
 *  again then. */
const autoFocusedRequests = new Set<string>();

const SCOPE_ITEMS: ReadonlyArray<{
  scope: Exclude<PermissionScope, "once">;
  label: string;
  where: string;
}> = [
  { scope: "session", label: "For this session", where: "not saved" },
  { scope: "project", label: "For this project", where: ".claude/settings.local.json" },
  { scope: "user", label: "For all projects", where: "~/.claude/settings.json" },
];

function ApprovalFooter({
  requestId,
  inputText,
  onDecide,
  toolName,
  toolInput,
}: ApprovalFooterProps) {
  const [denying, setDenying] = useState(false);
  const [reason, setReason] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const allowRef = useRef<HTMLButtonElement | null>(null);
  // Synchronous in-flight guard. The parent flips
  // `approval.resolution` to `responding` after the IPC round-trips,
  // but rapid double-clicks can fire `handleAllow` (or `confirmDeny`)
  // in the same tick before React re-renders. Once we've dispatched
  // a decision for this approval, all further clicks are dropped —
  // the footer is about to unmount when `responding` lands.
  const dispatchedRef = useRef(false);

  const scopedRule = useMemo(
    () => suggestPermissionRule(toolName, toolInput),
    [toolName, toolInput],
  );
  const anyInputRule: PermissionRuleSpec = { toolName };
  // Settings rules (`Bash(git status:*)`) are Claude's model; other
  // providers drop `updated_permissions`. They get the session-wide allow
  // they do support instead of rules that would silently do nothing.
  const provider = useChatProvider();
  const claudeRules = provider === null || provider === "claude";

  // Take keyboard focus only when nothing else holds it. A focused
  // composer, even an empty one, is where the user is about to type, and
  // an Enter landing on Allow there would approve by accident.
  useEffect(() => {
    if (autoFocusedRequests.has(requestId)) return;
    autoFocusedRequests.add(requestId);
    // Only recent requests can remount; keep the set bounded.
    if (autoFocusedRequests.size > 64) {
      const oldest = autoFocusedRequests.values().next().value;
      if (oldest !== undefined) autoFocusedRequests.delete(oldest);
    }
    const active = document.activeElement;
    if (active && active !== document.body) return;
    allowRef.current?.focus({ preventScroll: true });
  }, [requestId]);

  const handleAllow = (
    scope: PermissionScope,
    rule: PermissionRuleSpec = anyInputRule,
  ) => {
    if (dispatchedRef.current) return;
    const updatedPermissions = buildPermissionUpdate(scope, rule);
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
    // currently surface a write-failed signal. The exact rule and the
    // settings-file path are shown so the user can verify both.
    const ruleText = formatPermissionRule(rule);
    if (scope === "session") {
      toast.success(`Allowing ${ruleText} for this session`, {
        description: "Not saved to settings",
      });
    } else if (scope === "project") {
      toast.success(`Allowing ${ruleText} for this project`, {
        description: "Rule saved to .claude/settings.local.json",
      });
    } else if (scope === "user") {
      toast.success(`Allowing ${ruleText} for all projects`, {
        description: "Rule saved to ~/.claude/settings.json",
      });
    }
  };

  const allowForSession = () => {
    if (dispatchedRef.current) return;
    dispatchedRef.current = true;
    onDecide({ decision: "allow_for_session" });
  };

  const confirmDeny = () => {
    if (dispatchedRef.current) return;
    dispatchedRef.current = true;
    onDecide({ decision: "deny", message: reason || "User denied" });
  };

  const cancelDeny = () => {
    setDenying(false);
    setReason("");
  };

  // Single-key shortcuts while focus is inside the footer: Enter already
  // activates the focused Allow button, A opens the Allow-always menu and
  // D starts a denial.
  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    // Keys typed in the open Allow-always menu bubble here through the
    // React tree (the menu is portaled, not a DOM child). They belong to
    // the menu's own navigation and typeahead.
    if (menuOpen || denying || e.metaKey || e.ctrlKey || e.altKey) return;
    const key = e.key.toLowerCase();
    if (key === "a") {
      e.preventDefault();
      setMenuOpen(true);
    } else if (key === "d") {
      e.preventDefault();
      setDenying(true);
    }
  };

  return (
    <div
      className="group/approval border-t border-border/60 p-3 space-y-2"
      onKeyDown={handleKeyDown}
    >
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
            ref={allowRef}
            type="button"
            size="sm"
            className="bg-foreground text-background hover:bg-foreground/90"
            onClick={() => handleAllow("once")}
          >
            Allow
          </Button>
          <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="sm"
              >
                Allow always
                <ChevronDown className="ml-1 size-3" aria-hidden />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="start"
              className={cn("text-label", claudeRules ? "w-80" : "w-48")}
            >
              {!claudeRules && (
                <DropdownMenuItem
                  onSelect={allowForSession}
                  className="text-label"
                >
                  For this session
                </DropdownMenuItem>
              )}
              {/* The narrow rule leads; the any-input rule sits last and
                  says plainly how much it covers. */}
              {claudeRules && scopedRule && (
                <>
                  <RuleScopeGroup
                    rule={scopedRule}
                    onPick={(scope) => handleAllow(scope, scopedRule)}
                  />
                  <DropdownMenuSeparator />
                </>
              )}
              {claudeRules && (
                <RuleScopeGroup
                  rule={anyInputRule}
                  anyInput
                  onPick={(scope) => handleAllow(scope, anyInputRule)}
                />
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setDenying(true)}
          >
            Deny
          </Button>
          <span
            data-testid="approval-key-hints"
            className="ml-auto hidden text-caption text-muted-foreground/70 group-focus-within/approval:inline"
          >
            <Kbd>↵</Kbd> allow · <Kbd>A</Kbd> always · <Kbd>D</Kbd> deny
          </span>
        </div>
      ) : (
        <div className="space-y-2">
          <textarea
            autoFocus
            value={reason}
            onChange={(e) => setReason(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                cancelDeny();
              }
            }}
            placeholder="Reason (optional)"
            className="w-full resize-none rounded-md bg-background px-2 py-1.5 text-label text-foreground outline-none ring-1 ring-border focus:ring-muted-foreground/60"
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
              onClick={confirmDeny}
            >
              Confirm deny
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-foreground"
              onClick={cancelDeny}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

/** One rule and the three places it can live. The rule is printed exactly
 *  as it will be written, so "Allow always" never hides how much it
 *  allows. */
function RuleScopeGroup({
  rule,
  anyInput = false,
  onPick,
}: {
  rule: PermissionRuleSpec;
  anyInput?: boolean;
  onPick: (scope: Exclude<PermissionScope, "once">) => void;
}) {
  const ruleText = formatPermissionRule(rule);
  return (
    <DropdownMenuGroup aria-label={ruleText}>
      <DropdownMenuLabel className="flex min-w-0 items-baseline gap-2">
        <span
          className={cn(
            "min-w-0 truncate font-mono text-caption",
            anyInput ? "text-muted-foreground" : "text-foreground",
          )}
          title={ruleText}
        >
          {ruleText}
        </span>
        {anyInput && (
          <span className="shrink-0 text-caption text-muted-foreground">
            {rule.toolName === "Bash" ? "any command" : "any input"}
          </span>
        )}
      </DropdownMenuLabel>
      {SCOPE_ITEMS.map(({ scope, label, where }) => (
        <DropdownMenuItem
          key={scope}
          onSelect={() => onPick(scope)}
          className="text-label gap-3"
        >
          <span>{label}</span>
          <span className="ml-auto text-caption text-muted-foreground">
            {where}
          </span>
        </DropdownMenuItem>
      ))}
    </DropdownMenuGroup>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="rounded-sm bg-muted/60 px-1 py-[1px] font-mono text-caption text-muted-foreground/80">
      {children}
    </kbd>
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
  if (states.isPendingApproval) return { Icon: Clock, className: "text-status-working" };
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
    case "provider_option":
      return `Hermes permission: ${decision.option_id}`;
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
