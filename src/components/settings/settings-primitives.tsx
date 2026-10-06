/**
 * Layout primitives shared by the settings shell and the sections
 * extracted out of it.
 *
 * They live here rather than in `settings-view.tsx` so an extracted
 * section can render the real heading instead of a copy of its markup —
 * importing it back out of the shell would make the two files a cycle.
 */

import {
  Children,
  cloneElement,
  isValidElement,
  useId,
  useRef,
  type KeyboardEvent,
} from "react";
import { Monitor } from "lucide-react";
import { cn } from "@/lib/utils";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import { Select, SelectTrigger } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/** The title of a whole settings page. Every section renders this one
 *  heading so the title keeps its size and weight as you move between
 *  sections. `action` holds page-level buttons or a master toggle; `icon`
 *  sits before the title. */
export function SectionHeader({
  title,
  description,
  action,
  icon,
  className,
}: {
  title: string;
  description?: React.ReactNode;
  action?: React.ReactNode;
  icon?: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("mb-7 flex items-start justify-between gap-4", className)}>
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          {icon}
          <h2 className="text-title font-bold tracking-tight text-foreground">{title}</h2>
        </div>
        {description && (
          <p className="mt-1.5 max-w-prose text-body-lg leading-relaxed text-muted-foreground/80">
            {description}
          </p>
        )}
      </div>
      {action && <div className="flex shrink-0 items-center gap-2">{action}</div>}
    </div>
  );
}

type ControlLabelProps = { id: string; "aria-describedby"?: string };

/** Give a row's control the row's label and description.
 *
 *  Only controls that put `id` on a labelable element are wired: a Switch
 *  or Input directly, or a Select's trigger. Anything else (a picker, a
 *  read-only value, a button with its own text) keeps its own name, and
 *  the row becomes a labelled group instead. */
function labelControl(
  child: React.ReactNode,
  props: ControlLabelProps,
): { node: React.ReactNode; labelled: boolean } {
  if (
    !isValidElement<{ id?: string; children?: React.ReactNode }>(child) ||
    child.props.id
  ) {
    return { node: child, labelled: false };
  }
  if (child.type === Switch || child.type === Input) {
    return { node: cloneElement(child, props), labelled: true };
  }
  if (child.type === Select) {
    let labelled = false;
    const parts = Children.map(child.props.children, (part) => {
      if (labelled || !isValidElement<ControlLabelProps>(part) || part.type !== SelectTrigger) {
        return part;
      }
      labelled = true;
      return cloneElement(part, props);
    });
    return { node: cloneElement(child, undefined, parts), labelled };
  }
  return { node: child, labelled: false };
}

/** A small mark for a setting stored on this machine only. Everything
 *  without it follows your account to other devices. */
function DeviceOnlyMark({ id }: { id: string }) {
  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <span className="inline-flex text-muted-foreground/60">
            <Monitor className="size-3" aria-hidden />
            <span id={id} className="sr-only">
              Only on this device
            </span>
          </span>
        </TooltipTrigger>
        <TooltipContent>Only on this device. Not synced to your account.</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

/** One setting: label and description on the left, its control on the
 *  right. The label names the control for assistive tech, and
 *  `scope="device"` marks a setting that does not sync. */
export function SettingRow({
  label,
  description,
  scope,
  children,
}: {
  label: string;
  description?: string;
  scope?: "device";
  children: React.ReactNode;
}) {
  const id = useId();
  const controlId = `${id}-control`;
  const labelId = `${id}-label`;
  const descriptionId = description ? `${id}-description` : undefined;
  const scopeId = scope ? `${id}-scope` : undefined;
  const describedBy = [descriptionId, scopeId].filter(Boolean).join(" ") || undefined;
  const control = labelControl(children, {
    id: controlId,
    "aria-describedby": describedBy,
  });
  return (
    <div
      role={control.labelled ? undefined : "group"}
      aria-labelledby={control.labelled ? undefined : labelId}
      aria-describedby={control.labelled ? undefined : describedBy}
      className="flex items-center justify-between gap-8 py-4"
    >
      <div className="min-w-0 space-y-1">
        <div className="flex items-center gap-1.5">
          <label
            id={labelId}
            htmlFor={control.labelled ? controlId : undefined}
            className="text-body-lg leading-tight font-semibold text-foreground"
          >
            {label}
          </label>
          {scopeId && <DeviceOnlyMark id={scopeId} />}
        </div>
        {description && (
          <p id={descriptionId} className="text-body-sm leading-relaxed text-muted-foreground/80">
            {description}
          </p>
        )}
      </div>
      <div className="shrink-0">{control.node}</div>
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
    <div className={cn("mb-3 flex items-end justify-between gap-4", className)}>
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
  const optionRefs = useRef<(HTMLButtonElement | null)[]>([]);
  // One tab stop for the whole group (the checked option, or the first
  // when nothing is checked); arrow keys move the selection, as in a
  // native radio group.
  const selectedIndex = options.findIndex((opt) => opt.value === value);
  const tabStop = selectedIndex === -1 ? 0 : selectedIndex;
  const moveSelection = (event: KeyboardEvent, index: number) => {
    const last = options.length - 1;
    let next: number;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        next = index === last ? 0 : index + 1;
        break;
      case "ArrowLeft":
      case "ArrowUp":
        next = index === 0 ? last : index - 1;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = last;
        break;
      default:
        return;
    }
    event.preventDefault();
    onChange(options[next].value);
    optionRefs.current[next]?.focus();
  };
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className="inline-flex items-center gap-0.5 rounded-lg border border-border bg-muted/30 p-0.5"
    >
      {options.map((opt, index) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            ref={(node) => {
              optionRefs.current[index] = node;
            }}
            tabIndex={index === tabStop ? 0 : -1}
            onKeyDown={(event) => moveSelection(event, index)}
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
