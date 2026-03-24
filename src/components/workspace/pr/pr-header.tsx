import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ExternalLink, Copy, GitPullRequest } from "lucide-react";
import type { PullRequestInfo } from "@/tauri/types";

const STATE_COLORS: Record<string, string> = {
  OPEN: "bg-success/20 text-success",
  DRAFT: "bg-muted text-muted-foreground",
  MERGED: "bg-purple-500/20 text-purple-400",
  CLOSED: "bg-danger/20 text-danger",
};

const REVIEW_LABELS: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "Approved", cls: "text-success" },
  CHANGES_REQUESTED: { label: "Changes requested", cls: "text-warning" },
  REVIEW_REQUIRED: { label: "Review pending", cls: "text-muted-foreground" },
};

function formatRelativeTime(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 30) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

interface Props {
  pr: PullRequestInfo;
}

export function PrHeader({ pr }: Props) {
  const [copied, setCopied] = useState(false);

  const stateLabel = pr.is_draft ? "Draft" : (pr.state ?? "OPEN");
  const stateColorCls =
    STATE_COLORS[stateLabel.toUpperCase()] ?? STATE_COLORS.OPEN;
  const review = pr.review_decision ? REVIEW_LABELS[pr.review_decision] : null;

  const handleCopyUrl = () => {
    navigator.clipboard.writeText(pr.url);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="space-y-1.5">
      {/* State + Number */}
      <div className="flex items-center gap-1.5">
        <GitPullRequest className="h-3.5 w-3.5 text-muted-foreground" />
        <Badge className={`text-[10px] px-1.5 py-0 ${stateColorCls}`}>
          {stateLabel}
        </Badge>
        <span className="text-[10px] text-muted-foreground">
          #{pr.number}
        </span>
      </div>

      {/* Title */}
      <p className="text-xs font-medium text-foreground leading-snug">
        {pr.title}
      </p>

      {/* Branches */}
      {pr.base_branch && pr.head_branch && (
        <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
          <span className="font-mono bg-muted px-1 rounded">{pr.head_branch}</span>
          <span>&rarr;</span>
          <span className="font-mono bg-muted px-1 rounded">{pr.base_branch}</span>
        </div>
      )}

      {/* Stats row */}
      <div className="flex items-center gap-2 text-[10px]">
        {pr.additions != null && (
          <span className="text-success">+{pr.additions}</span>
        )}
        {pr.deletions != null && (
          <span className="text-danger">-{pr.deletions}</span>
        )}
        {review && <span className={review.cls}>{review.label}</span>}
        {pr.updated_at && (
          <span className="text-muted-foreground">
            {formatRelativeTime(pr.updated_at)}
          </span>
        )}
      </div>

      {/* Actions */}
      <div className="flex items-center gap-1.5">
        <a
          href={pr.url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-[10px] text-primary hover:underline"
          onClick={(e) => {
            e.preventDefault();
            window.open(pr.url, "_blank");
          }}
        >
          <ExternalLink className="h-3 w-3" />
          GitHub
        </a>
        <Button
          size="xs"
          variant="ghost"
          className="text-[10px] h-5 px-1.5"
          onClick={handleCopyUrl}
        >
          <Copy className="h-2.5 w-2.5 mr-0.5" />
          {copied ? "Copied" : "Copy URL"}
        </Button>
      </div>
    </div>
  );
}
