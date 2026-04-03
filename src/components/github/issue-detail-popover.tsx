import { useState, useEffect } from "react";
import { cn } from "@/lib/utils";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { CircleDot, CircleCheck, ExternalLink } from "lucide-react";
import { getGithubIssue } from "@/tauri/commands";
import type { LinkedIssue, GitHubIssue } from "@/tauri/types";

interface Props {
  workspaceId: string;
  issue: LinkedIssue;
}

export function IssueDetailPopover({ workspaceId, issue }: Props) {
  const [open, setOpen] = useState(false);
  const [fullIssue, setFullIssue] = useState<GitHubIssue | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchIssue = () => {
    setLoading(true);
    setError(null);
    setFullIssue(null);
    getGithubIssue(workspaceId, issue.number)
      .then(setFullIssue)
      .catch((err) => setError(String(err)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    if (open) fetchIssue();
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1 shrink-0 hover:text-foreground transition-colors"
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") e.stopPropagation(); }}
        >
          <span
            className={cn(
              "size-1.5 rounded-full",
              issue.state === "Open" ? "bg-success" : "bg-muted-foreground",
            )}
          />
          <span className="text-[10px] tabular-nums">
            #{issue.number}
          </span>
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="right"
        align="start"
        sideOffset={8}
        className="w-[380px] p-0"
      >
        <div className="p-4 space-y-3" data-testid="issue-detail-content">
          {loading ? (
            <div className="space-y-3">
              <Skeleton className="h-4 w-3/4" />
              <div className="flex gap-2">
                <Skeleton className="h-3 w-12" />
                <Skeleton className="h-3 w-16" />
              </div>
              <Separator />
              <Skeleton className="h-20 w-full" />
            </div>
          ) : error ? (
            <div className="text-center space-y-2 py-2">
              <p className="text-xs text-muted-foreground">Failed to load issue details</p>
              <Button
                variant="ghost"
                size="sm"
                className="h-7 text-xs"
                onClick={fetchIssue}
              >
                Retry
              </Button>
            </div>
          ) : fullIssue ? (
            <>
              {/* Header: icon + title */}
              <div className="flex items-start gap-2">
                {fullIssue.state === "Open" ? (
                  <CircleDot className="size-4 shrink-0 text-success mt-0.5" />
                ) : (
                  <CircleCheck className="size-4 shrink-0 text-muted-foreground mt-0.5" />
                )}
                <p className="text-sm font-medium text-foreground leading-snug line-clamp-2">
                  {fullIssue.title}
                </p>
              </div>

              {/* Meta: number + labels + assignees */}
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-xs text-muted-foreground font-mono">
                  #{fullIssue.number}
                </span>
                {fullIssue.labels.map((label) => (
                  <Badge
                    key={label}
                    variant="secondary"
                    className="text-[10px] px-1.5 py-0"
                  >
                    {label}
                  </Badge>
                ))}
                {fullIssue.assignees.length > 0 && (
                  <span className="text-[10px] text-muted-foreground">
                    {fullIssue.assignees.join(", ")}
                  </span>
                )}
              </div>

              <Separator />

              {/* Body */}
              {fullIssue.body ? (
                <div className="max-h-[300px] overflow-y-auto">
                  <p className="text-xs text-muted-foreground whitespace-pre-wrap break-words leading-relaxed">
                    {fullIssue.body}
                  </p>
                </div>
              ) : (
                <p className="text-xs text-muted-foreground italic">
                  No description provided.
                </p>
              )}

              {/* Open on GitHub */}
              {fullIssue.url && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="w-full justify-center gap-1.5 text-xs"
                  onClick={() => window.open(fullIssue.url, "_blank")}
                >
                  <ExternalLink className="h-3 w-3" />
                  Open on GitHub
                </Button>
              )}
            </>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}
