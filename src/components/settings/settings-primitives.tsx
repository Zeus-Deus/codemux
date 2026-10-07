/**
 * Layout primitives shared by the settings shell and the sections
 * extracted out of it.
 *
 * They live here rather than in `settings-view.tsx` so an extracted
 * section can render the real heading instead of a copy of its markup —
 * importing it back out of the shell would make the two files a cycle.
 */

import { cn } from "@/lib/utils";
import { Eyebrow } from "@/components/ui/eyebrow";

/** Title and summary at the top of a settings page. */
export function SectionHeader({ title, description }: { title: string; description: string }) {
  return (
    <div className="mb-7">
      <h2 className="text-[1.3125rem] font-bold tracking-tight text-foreground">{title}</h2>
      <p className="text-body-lg text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">{description}</p>
    </div>
  );
}

/** One setting: label and description on the left, its control on the
 *  right. `data-settings-row` marks what Settings search highlights when it
 *  lands on this row. */
export function SettingRow({ label, description, children }: {
  label: string;
  description?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div data-settings-row className="flex items-center justify-between gap-8 py-4">
      <div className="min-w-0 space-y-1">
        <p className="text-body-lg leading-tight font-semibold text-foreground">{label}</p>
        {description && (
          <p className="text-body-sm leading-relaxed text-muted-foreground/80">{description}</p>
        )}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

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
    <div data-settings-row className={cn("mb-3 flex items-end justify-between gap-4", className)}>
      <div className="min-w-0">
        <Eyebrow>
          {title}
        </Eyebrow>
        {description && (
          <p className="text-body-sm text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">
            {description}
          </p>
        )}
      </div>
      {action && <div className="shrink-0">{action}</div>}
    </div>
  );
}

/** Segmented control — a bordered pill of mutually-exclusive options
 *  with a neutral foreground-filled active segment (the design system's
 *  "white is the baseline" rule for toggles/selection).
 *
 *  Lifted out of `settings-view.tsx` when the Usage section needed the
 *  same control for its period and metric pickers. Importing it back
 *  out of the shell would make the two files a cycle — the reason this
 *  module exists. */
export function SegmentedControl<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
  size = "md",
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: string }[];
  ariaLabel?: string;
  /** `sm` is the in-card variant (the Cost/Tokens metric toggle), which
   *  sits beside content rather than titling it. */
  size?: "sm" | "md";
}) {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className="inline-flex items-center gap-0.5 rounded-lg border border-border bg-muted/30 p-0.5"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              "rounded-md font-medium transition-colors duration-150",
              size === "sm"
                ? "px-2.5 py-0.5 text-label"
                : "px-3 py-1 text-body-sm",
              active
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
