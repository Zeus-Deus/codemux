import { useMemo, memo } from "react";
import { CheckCircle2, XCircle, Loader2, Minus } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import type { CheckInfo } from "@/tauri/types";
import { CollapsibleSection } from "./collapsible-section";

// Per-row check icon. Matches the tab-badge spinner so the same color
// + motion appears in both places when checks are running.
//
// `memo` is load-bearing here: React Query refetches every 2.5s and
// hands ReviewChecks fresh `checks` array references each tick. Without
// memo, every CheckIcon would re-render — even when the status hasn't
// changed — and the CSS `animate-spin` keyframe would visibly stutter
// every couple of seconds. With memo + a primitive-string `status`
// prop, React skips the re-render entirely and the SVG keeps spinning
// smoothly through the refetch.
const CheckIcon = memo(function CheckIcon({ status }: { status: string }) {
  const s = status.toLowerCase();
  if (s === "pass" || s === "success")
    return <CheckCircle2 className="h-3 w-3 text-success shrink-0" />;
  if (s === "fail" || s === "failure")
    return <XCircle className="h-3 w-3 text-danger shrink-0" />;
  if (s === "skipping" || s === "skipped" || s === "cancel" || s === "cancelled")
    return <Minus className="h-3 w-3 text-muted-foreground shrink-0" />;
  // pending / queued / in-progress / anything else → spinning amber loader
  return <Loader2 className="h-3 w-3 text-status-working shrink-0 animate-spin" />;
});

interface Props {
  checks: CheckInfo[];
  isLoading?: boolean;
}

export function ReviewChecks({ checks, isLoading = false }: Props) {
  const { passed, total, summaryColor } = useMemo(() => {
    const p = checks.filter(
      (c) => {
        const s = (c.conclusion ?? c.status).toLowerCase();
        return s === "success" || s === "pass";
      },
    ).length;
    const t = checks.length;
    const failed = checks.some((c) => {
      const s = (c.conclusion ?? c.status).toLowerCase();
      return s === "failure" || s === "fail";
    });
    const color = p === t ? "text-success" : failed ? "text-danger" : "text-warning";
    return { passed: p, total: t, summaryColor: color };
  }, [checks]);

  // Right-side summary in the section header. Empty state and loading
  // state both keep the section visible (per Superset) but suppress the
  // "X/Y" badge when there's nothing to summarize.
  const summaryNode = total > 0 ? (
    <span className={`text-[10px] ${summaryColor}`}>
      {passed}/{total} checks passing
    </span>
  ) : null;

  return (
    <CollapsibleSection label="Checks" count={total} rightSlot={summaryNode}>
      <div className="px-1.5 space-y-0.5">
        {checks.length > 0 ? (
          checks.map((check) => (
            <div
              key={check.name}
              className="flex items-center gap-1.5 py-0.5 px-1"
            >
              <CheckIcon status={check.conclusion ?? check.status} />
              <span className="text-xs text-foreground truncate flex-1">
                {check.detail_url ? (
                  <a
                    href={check.detail_url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="hover:underline"
                    onClick={(e) => {
                      e.preventDefault();
                      window.open(check.detail_url!, "_blank");
                    }}
                  >
                    {check.name}
                  </a>
                ) : (
                  check.name
                )}
              </span>
              {check.elapsed_time && (
                <span className="text-[10px] text-muted-foreground shrink-0">
                  {check.elapsed_time}
                </span>
              )}
            </div>
          ))
        ) : isLoading ? (
          <>
            <Skeleton className="h-4 w-2/3 mx-1 my-0.5" />
            <Skeleton className="h-4 w-1/2 mx-1 my-0.5" />
            <Skeleton className="h-4 w-3/5 mx-1 my-0.5" />
          </>
        ) : (
          <p className="text-xs text-muted-foreground px-1 py-0.5">
            No checks reported.
          </p>
        )}
      </div>
    </CollapsibleSection>
  );
}
