import { cn } from "@/lib/utils";
import { useMemo } from "react";
import { GitPullRequest, type LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useUIStore } from "@/stores/ui-store";
import { usePrOverview } from "@/lib/pr-overview-query";
import { badgeCount, badgeKeys, badgeLabel } from "@/lib/pr-overview";

/**
 * The Pull Requests destination, with the only badge Codemux raises for
 * pull requests.
 *
 * It counts two things and no others: a review someone is waiting on you
 * for, and one of your own pull requests having gone red. Everything
 * else a pull request can do is something you will find when you look,
 * and a badge that counts those is a badge you learn to ignore.
 *
 * The query lives here rather than on the page so the count exists
 * before the page has ever been opened; it shares its key with the
 * page's own fetch, so opening it costs nothing extra.
 */
export function SidebarPullRequestsButton({
  tooltipSide = "top",
  labeled = false,
  icon: Icon = GitPullRequest,
}: {
  tooltipSide?: "top" | "right";
  icon?: LucideIcon;
  labeled?: boolean;
}) {
  const setShowPullRequests = useUIStore((s) => s.setShowPullRequests);
  const seen = useUIStore((s) => s.prBadgeSeen);
  const { rows, viewerByRoot } = usePrOverview(true);

  const count = useMemo(
    () => badgeCount(badgeKeys(rows, viewerByRoot), new Set(seen)),
    [rows, viewerByRoot, seen],
  );

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Pull requests"
          data-testid="sidebar-pull-requests"
          onClick={() => setShowPullRequests(true)}
          className={cn("relative h-7 rounded-[7px] text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground", labeled ? "w-full justify-start gap-2 px-2 text-xs" : "w-7")}
        >
          <Icon className="size-[15px]" />
          {labeled && "Pull requests"}
          {count > 0 && (
            <span
              data-testid="sidebar-pull-requests-badge"
              className="absolute -right-0.5 -top-0.5 min-w-[15px] h-[15px] px-1 rounded-full bg-primary text-center text-[9px] leading-[15px] text-primary-foreground font-semibold tabular-nums"
            >
              {badgeLabel(count)}
            </span>
          )}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide} sideOffset={4} className="text-xs">
        {count > 0
          ? `Pull requests — ${count} waiting on you`
          : "Pull requests"}
      </TooltipContent>
    </Tooltip>
  );
}
