import { ArrowUpRight } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import { PrStatusIcon } from "@/components/github/pr-status-icon";
import type { PullRequestInfo } from "@/tauri/types";

interface ReviewDecisionConfig {
  label: string;
  className: string;
}

// Mirrors Superset's reviewDecisionConfig — state-coloured pill style
// matching the existing Codemux semantic tokens.
const REVIEW_DECISION_CONFIG: Record<string, ReviewDecisionConfig> = {
  APPROVED: {
    label: "Approved",
    className: "border border-emerald-500/20 bg-emerald-500/10 text-emerald-400",
  },
  CHANGES_REQUESTED: {
    label: "Changes requested",
    className: "border border-red-500/20 bg-red-500/10 text-red-400",
  },
  REVIEW_REQUIRED: {
    label: "Review pending",
    className: "border border-amber-500/20 bg-amber-500/10 text-amber-400",
  },
};

const DEFAULT_DECISION: ReviewDecisionConfig = REVIEW_DECISION_CONFIG.REVIEW_REQUIRED;

interface Props {
  pr: PullRequestInfo;
}

// Minimal header that mirrors Superset's resting layout (analysis §6
// Feature 3, screenshot reference). The chrome that used to live here —
// state pill, PR number, branch arrows, diff stats, age, GitHub link,
// Copy URL — was redundant once the sidebar PR icon, the Changes-panel
// "open PR" button, and the Diffs tab carry the same info. Title is
// the click target now (right-click → Copy Link Address replaces the
// removed Copy URL button).
export function ReviewHeader({ pr }: Props) {
  const decision: ReviewDecisionConfig =
    !pr.is_draft && pr.review_decision
      ? REVIEW_DECISION_CONFIG[pr.review_decision] ?? DEFAULT_DECISION
      : DEFAULT_DECISION;

  const handleOpen = (e: React.MouseEvent) => {
    e.preventDefault();
    openUrl(pr.url).catch(console.error);
  };

  return (
    <div className="space-y-1.5">
      <a
        href={pr.url}
        target="_blank"
        rel="noopener noreferrer"
        onClick={handleOpen}
        className="group flex items-center gap-1.5 cursor-pointer"
      >
        <PrStatusIcon
          state={pr.is_draft ? "draft" : pr.state}
          size={4}
          className="shrink-0"
        />
        <span
          className="min-w-0 flex-1 truncate text-xs font-medium text-foreground"
          title={pr.title}
        >
          {pr.title}
        </span>
        <ArrowUpRight className="size-3.5 shrink-0 text-muted-foreground/70 opacity-0 transition-opacity group-hover:opacity-100" />
      </a>
      <div>
        <span
          className={cn(
            "inline-block shrink-0 rounded-sm px-1.5 py-0.5 text-[10px] font-medium",
            decision.className,
          )}
        >
          {decision.label}
        </span>
      </div>
    </div>
  );
}
