import { cn } from "@/lib/utils";
import { tzBodyLg, tzEyebrow } from "./review-ui";

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
  /**
   * Right-hand slot, for a control that belongs to the active tab rather
   * than to the strip — currently the Timeline's filter.
   *
   * It lives here rather than inside the tab body because the strip is
   * the one row whose height is already fixed: putting the filter in the
   * body would push the first entry of every other tab down by its
   * height, or make the tabs change height between tabs.
   */
  trailing?: React.ReactNode;
}

/**
 * Summary / Timeline / Code.
 *
 * Takes a tab list rather than hard-coding its three tabs so the later
 * ships can add Code and Timeline without the strip — or anything below
 * it — changing height. A single-tab strip still renders its underline,
 * which is why the panel's resting geometry is already final.
 */
export function ReviewTabStrip({ tabs, activeId, onSelect, trailing }: Props) {
  // Roving focus: Tab lands on the active tab, arrows (and Home/End)
  // move between tabs and switch with them — a tab switch here is cheap.
  const onKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    const index = tabs.findIndex((tab) => tab.id === activeId);
    const next =
      event.key === "ArrowRight"
        ? (index + 1) % tabs.length
        : event.key === "ArrowLeft"
          ? (index - 1 + tabs.length) % tabs.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? tabs.length - 1
              : null;
    if (next == null || !tabs[next]) return;
    event.preventDefault();
    onSelect(tabs[next].id);
    event.currentTarget.parentElement
      ?.querySelector<HTMLElement>(`[data-testid="review-tab-${tabs[next].id}"]`)
      ?.focus();
  };

  return (
    <div
      role="tablist"
      className="flex items-center gap-4 border-b border-border/40 px-4"
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
            tabIndex={active ? 0 : -1}
            data-testid={`review-tab-${tab.id}`}
            onClick={() => onSelect(tab.id)}
            onKeyDown={onKeyDown}
            className={cn(
              "-mb-px border-b-[1.5px] py-2.5 transition-colors duration-150",
              tzBodyLg,
              active
                ? "border-accent-ember font-semibold text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground",
            )}
          >
            {tab.label}
            {tab.count != null && (
              <span className={cn("ml-1.5 font-mono", tzEyebrow)}>{tab.count}</span>
            )}
          </button>
        );
      })}
      {trailing && (
        <>
          <span className="flex-1" />
          {trailing}
        </>
      )}
    </div>
  );
}
