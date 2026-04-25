import { useState } from "react";
import { ChevronRight } from "lucide-react";

interface Props {
  label: string;
  count?: number | string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}

export function CollapsibleSection({
  label,
  count,
  defaultOpen = true,
  children,
}: Props) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <div className="py-1">
      <button
        className="flex w-full items-center justify-between px-1.5 py-0.5 hover:bg-accent/30 rounded-sm transition-colors"
        onClick={() => setOpen(!open)}
      >
        <div className="flex items-center gap-1">
          <ChevronRight
            className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${open ? "rotate-90" : ""}`}
          />
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            {label}
          </span>
        </div>
        {count !== undefined && (
          <span className="text-[10px] tabular-nums text-muted-foreground">
            {count}
          </span>
        )}
      </button>
      {open && children}
    </div>
  );
}
