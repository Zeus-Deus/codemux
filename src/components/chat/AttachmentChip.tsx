import {
  ChevronsUpDown,
  CircleDot,
  File as FileIcon,
  FolderOpen,
  GitPullRequest,
  Image as ImageIcon,
  Loader2,
  X,
  type LucideIcon,
} from "lucide-react";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { Attachment, AttachmentKind } from "@/stores/agent-chat-store";

interface KindConfig {
  icon: LucideIcon;
  /** Tailwind classes — bg + text. 15% opacity fill matches ModePill
   *  per the chat-ui chip token. Note: chat-ui skill technically
   *  reserves accent for the app shell only, but ModePill set the
   *  in-pane precedent and AttachmentChip mirrors it. */
  className: string;
}

/** Per-kind visual config. Issue/PR colors are deliberately *open*
 *  state — closed/merged variants are computed at render time so the
 *  same component handles both. */
const KIND_CONFIG: Record<AttachmentKind, KindConfig> = {
  file: {
    icon: FileIcon,
    className: "bg-foreground/10 text-foreground border-border/60",
  },
  folder: {
    icon: FolderOpen,
    className: "bg-foreground/10 text-foreground border-border/60",
  },
  issue: {
    icon: CircleDot,
    className: "bg-warning/15 text-warning border-warning/25",
  },
  pr: {
    icon: GitPullRequest,
    className: "bg-primary/15 text-primary border-primary/25",
  },
  image: {
    icon: ImageIcon,
    className: "bg-accent/15 text-accent-foreground border-accent/30",
  },
};

/** State-aware tint resolution. Open issues + open non-draft PRs use
 *  their accent; everything else (closed, merged, draft) gets a
 *  muted treatment so the chip strip doesn't look like a status
 *  billboard. Merged PRs get a one-off purple fallback so a merged
 *  ref is still visually distinct from a closed/draft one — matches
 *  the picker's GitMerge tint. */
function classNameForAttachment(attachment: Attachment): string {
  const state = attachment.metadata.state;
  if (attachment.kind === "issue" && state === "closed") {
    return "bg-foreground/10 text-muted-foreground border-border/60";
  }
  if (attachment.kind === "pr") {
    if (state === "merged") {
      return "bg-chart-4/15 text-chart-4 border-chart-4/25";
    }
    if (state === "closed" || state === "draft") {
      return "bg-foreground/10 text-muted-foreground border-border/60";
    }
  }
  return KIND_CONFIG[attachment.kind].className;
}

/** Step 8 Stage 7 — rough token estimate for the tooltip preview.
 *  Heuristic: bytes/4 for code, bytes/5 for prose. Image gets a flat
 *  ~255 token estimate (3 tiles at ~85 tokens/tile per Claude's
 *  formula) — exact tile count is provider-specific so the number
 *  is intentionally a ballpark, not authoritative. Returns 0 when
 *  the attachment hasn't resolved yet so the tooltip can swap in a
 *  loading message. */
function estimateTokens(att: Attachment): number {
  if (att.kind === "image" && att.resolvedImage) {
    return 255;
  }
  if (att.resolvedContent) {
    const bytes = new TextEncoder().encode(att.resolvedContent).length;
    const label = att.metadata.label ?? "";
    const isCode =
      att.kind === "file" &&
      ["ts", "tsx", "rs", "py", "go", "js", "jsx", "java", "c", "cpp", "h"].some(
        (ext) => label.toLowerCase().endsWith(`.${ext}`),
      );
    return Math.round(bytes / (isCode ? 4 : 5));
  }
  return 0;
}

interface AttachmentChipProps {
  attachment: Attachment;
  onRemove: (id: string) => void;
  /** Step 8 Stage 7 — PR-only: toggle the full-diff flag on the
   *  attachment. Wired by AgentChatPane.tsx; the chip just renders
   *  the affordance. Optional so non-PR call sites stay terse. */
  onToggleExpand?: (id: string) => void;
}

/** Compact chip rendered in the strip above the composer textarea
 *  when an attachment is staged. Mirrors the ModePill chip shape and
 *  removal affordance.
 *
 *  Stage 1 only exercises `kind: "file"` end-to-end — the other kinds
 *  render via the same component so Stages 2–6 can flip their flows
 *  on without touching this file. */
export function AttachmentChip({
  attachment,
  onRemove,
  onToggleExpand,
}: AttachmentChipProps) {
  const Icon = KIND_CONFIG[attachment.kind].icon;
  const { metadata } = attachment;
  const isTruncatedFile =
    attachment.kind === "file" &&
    metadata.isTruncated === true &&
    typeof metadata.lineCount === "number";
  const lineCountLabel =
    typeof metadata.lineCount === "number" && !isTruncatedFile
      ? `${metadata.lineCount}L`
      : null;
  const truncationLabel = isTruncatedFile
    ? `first 50/${metadata.lineCount}L`
    : null;
  const tokenEstimate = estimateTokens(attachment);
  const showExpand =
    attachment.kind === "pr" &&
    typeof onToggleExpand === "function" &&
    !metadata.isLoading;
  const expandActive = metadata.expandFullDiff === true;
  const expandTooltip = expandActive
    ? "Show filenames only"
    : "Show full diff";

  const chip = (
    <div
      className={cn(
        // Bordered chip (design D3/D10): a matching-tint hairline gives
        // each staged ref the card look the design applies to the
        // green issue chip; the per-kind tint below sets both fill and
        // border colour.
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs",
        classNameForAttachment(attachment),
      )}
      role="status"
      aria-label={`${attachment.kind} attachment: ${metadata.label}`}
      data-attachment-kind={attachment.kind}
      data-truncated={isTruncatedFile || undefined}
      data-expanded={expandActive || undefined}
    >
      {metadata.isLoading ? (
        <Loader2
          className="h-3 w-3 animate-spin"
          aria-hidden
          data-testid="attachment-chip-spinner"
        />
      ) : (
        <Icon className="h-3 w-3" aria-hidden />
      )}
      <span className="truncate max-w-[200px]">{metadata.label}</span>
      {lineCountLabel && (
        <span className="text-[10px] opacity-70" aria-hidden>
          {lineCountLabel}
        </span>
      )}
      {truncationLabel && (
        <span
          className="text-[9px] opacity-60"
          aria-hidden
          data-testid="attachment-chip-truncation"
        >
          {truncationLabel}
        </span>
      )}
      {metadata.error && (
        <span
          className="text-destructive text-[10px]"
          aria-label={`error: ${metadata.error}`}
          title={metadata.error}
        >
          !
        </span>
      )}
      {showExpand && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onToggleExpand?.(attachment.id);
          }}
          className={cn(
            "ml-0.5 rounded p-0.5 hover:bg-foreground/10",
            expandActive && "bg-foreground/10",
          )}
          aria-label={expandTooltip}
          aria-pressed={expandActive}
          title={expandTooltip}
          data-testid="attachment-chip-expand"
        >
          <ChevronsUpDown className="h-2.5 w-2.5" />
        </button>
      )}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onRemove(attachment.id);
        }}
        className="ml-0.5 rounded p-0.5 hover:bg-foreground/10"
        aria-label={`Remove ${metadata.label}`}
      >
        <X className="h-2.5 w-2.5" />
      </button>
    </div>
  );

  return (
    <TooltipProvider delayDuration={250}>
      <Tooltip>
        <TooltipTrigger asChild>{chip}</TooltipTrigger>
        <TooltipContent
          side="top"
          data-testid="attachment-chip-tooltip"
          className="flex-col items-start gap-0.5 py-2"
        >
          <div className="font-mono text-[11px] truncate max-w-[260px]">
            {metadata.label}
          </div>
          <div className="text-[10px] opacity-80">
            {metadata.isLoading
              ? "Resolving…"
              : tokenEstimate > 0
                ? `~${tokenEstimate.toLocaleString()} tokens`
                : "—"}
          </div>
          {attachment.kind === "file" && typeof metadata.lineCount === "number" && (
            <div className="text-[10px] opacity-70">
              {metadata.lineCount.toLocaleString()} lines
              {typeof metadata.bytes === "number"
                ? ` · ${metadata.bytes.toLocaleString()} bytes`
                : ""}
            </div>
          )}
          {attachment.kind === "image" && typeof metadata.bytes === "number" && (
            <div className="text-[10px] opacity-70">
              {(metadata.bytes / 1024).toLocaleString(undefined, {
                maximumFractionDigits: 1,
              })}{" "}
              KB
            </div>
          )}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
