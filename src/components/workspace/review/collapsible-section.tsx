import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import { tzMeta, tzRowTitle } from "./review-ui";

interface Props {
  label: string;
  /** Inline count badge next to the label on the left (e.g. "Checks 4"). */
  count?: number | string;
  /** Right-side header slot (e.g. "4/4 checks passing"). */
  rightSlot?: React.ReactNode;
  defaultOpen?: boolean;
  children: React.ReactNode;
}

export function CollapsibleSection({
  label,
  count,
  rightSlot,
  defaultOpen = true,
  children,
}: Props) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div className="py-1">
      <button
        className="flex w-full items-center justify-between px-1.5 py-1 hover:bg-accent/30 rounded-sm transition-colors"
        onClick={() => setOpen(!open)}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          <ChevronRight
            className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${open ? "rotate-90" : ""}`}
          />
          <span className={cn("font-medium text-foreground truncate", tzRowTitle)}>
            {label}
          </span>
          {count !== undefined && (
            <span className={cn("tabular-nums text-muted-foreground", tzMeta)}>
              {count}
            </span>
          )}
        </div>
        {rightSlot && <div className="shrink-0">{rightSlot}</div>}
      </button>
      {open && children}
    </div>
  );
}
