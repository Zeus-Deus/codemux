/**
 * Layout primitives shared by the settings shell and the sections
 * extracted out of it.
 *
 * They live here rather than in `settings-view.tsx` so an extracted
 * section can render the real heading instead of a copy of its markup —
 * importing it back out of the shell would make the two files a cycle.
 */

import { cn } from "@/lib/utils";

/** In-section heading for grouped content (e.g. "AI Tools",
 *  "Detected editors"). Distinct from SectionHeader (which titles
 *  the whole panel) — lower visual weight, no max width, sits
 *  immediately above a stack of rows or a card. */
export function SubsectionHeader({
  title,
  description,
  action,
  className,
}: {
  title: string;
  description?: string;
  action?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("mb-3 flex items-end justify-between gap-4", className)}>
      <div className="min-w-0">
        <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
          {title}
        </p>
        {description && (
          <p className="text-[12px] text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">
            {description}
          </p>
        )}
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}
