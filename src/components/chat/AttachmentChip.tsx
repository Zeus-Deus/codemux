import {
  CircleDot,
  File as FileIcon,
  FolderOpen,
  GitPullRequest,
  Image as ImageIcon,
  Loader2,
  X,
  type LucideIcon,
} from "lucide-react";

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
    className: "bg-foreground/10 text-foreground",
  },
  folder: {
    icon: FolderOpen,
    className: "bg-foreground/10 text-foreground",
  },
  issue: {
    icon: CircleDot,
    className: "bg-warning/15 text-warning",
  },
  pr: {
    icon: GitPullRequest,
    className: "bg-primary/15 text-primary",
  },
  image: {
    icon: ImageIcon,
    className: "bg-accent/15 text-accent-foreground",
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
    return "bg-foreground/10 text-muted-foreground";
  }
  if (attachment.kind === "pr") {
    if (state === "merged") {
      return "bg-chart-4/15 text-chart-4";
    }
    if (state === "closed" || state === "draft") {
      return "bg-foreground/10 text-muted-foreground";
    }
  }
  return KIND_CONFIG[attachment.kind].className;
}

interface AttachmentChipProps {
  attachment: Attachment;
  onRemove: (id: string) => void;
}

/** Compact chip rendered in the strip above the composer textarea
 *  when an attachment is staged. Mirrors the ModePill chip shape and
 *  removal affordance.
 *
 *  Stage 1 only exercises `kind: "file"` end-to-end — the other kinds
 *  render via the same component so Stages 2–6 can flip their flows
 *  on without touching this file. */
export function AttachmentChip({ attachment, onRemove }: AttachmentChipProps) {
  const Icon = KIND_CONFIG[attachment.kind].icon;
  const { metadata } = attachment;
  const lineCountLabel =
    typeof metadata.lineCount === "number" ? `${metadata.lineCount}L` : null;
  return (
    <div
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs",
        classNameForAttachment(attachment),
      )}
      role="status"
      aria-label={`${attachment.kind} attachment: ${metadata.label}`}
      data-attachment-kind={attachment.kind}
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
      {metadata.error && (
        <span
          className="text-destructive text-[10px]"
          aria-label={`error: ${metadata.error}`}
          title={metadata.error}
        >
          !
        </span>
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
}
