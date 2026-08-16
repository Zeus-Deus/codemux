import { cn } from "@/lib/utils";

export interface ReviewTab {
  id: string;
  label: string;
  /** Mono count rendered after the label (e.g. `Code 8`). */
  count?: number | null;
}

interface Props {
  tabs: ReviewTab[];
  activeId: string;
  onSelect: (id: string) => void;
}

/**
 * Summary / Timeline / Code.
 *
 * Takes a tab list rather than hard-coding its three tabs so the later
 * ships can add Code and Timeline without the strip — or anything below
 * it — changing height. A single-tab strip still renders its underline,
 * which is why the panel's resting geometry is already final.
 */
export function ReviewTabStrip({ tabs, activeId, onSelect }: Props) {
  return (
    <div
      role="tablist"
      className="flex items-center gap-4 border-b border-border/40 px-3.5"
      data-testid="review-tab-strip"
    >
      {tabs.map((tab) => {
        const active = tab.id === activeId;
        return (
          <button
            key={tab.id}
            type="button"
            role="tab"
            aria-selected={active}
            data-testid={`review-tab-${tab.id}`}
            onClick={() => onSelect(tab.id)}
            className={cn(
              "-mb-px border-b-[1.5px] py-2 text-[11.5px] transition-colors",
              active
                ? "border-accent-ember font-semibold text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )}
          >
            {tab.label}
            {tab.count != null && (
              <span className="ml-1 font-mono text-[9.5px]">{tab.count}</span>
            )}
          </button>
        );
      })}
    </div>
  );
}
