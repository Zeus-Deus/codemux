import { useMemo } from "react";
import { CheckCircle2, XCircle, Clock } from "lucide-react";
import type { CheckInfo } from "@/tauri/types";
import { CollapsibleSection } from "./collapsible-section";

function CheckIcon({ status }: { status: string }) {
  if (status === "pass" || status === "success" || status === "SUCCESS")
    return <CheckCircle2 className="h-3 w-3 text-success shrink-0" />;
  if (status === "fail" || status === "failure" || status === "FAILURE")
    return <XCircle className="h-3 w-3 text-danger shrink-0" />;
  return <Clock className="h-3 w-3 text-warning shrink-0" />;
}

interface Props {
  checks: CheckInfo[];
}

export function PrChecks({ checks }: Props) {
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

  if (checks.length === 0) return null;

  return (
    <CollapsibleSection label="Checks" count={`${passed}/${total}`}>
      <div className="px-1.5 space-y-0.5">
        {/* Summary */}
        <p className={`text-[10px] ${summaryColor} px-1`}>
          {passed}/{total} checks passed
        </p>

        {/* Check list */}
        {checks.map((check) => (
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
        ))}
      </div>
    </CollapsibleSection>
  );
}
