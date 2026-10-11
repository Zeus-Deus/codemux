import { useState } from "react";
import { Brain, Check, ChevronDown, Zap } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { cn } from "@/lib/utils";
import type { ChatModelInfo } from "@/tauri/types";
import { focusCmdkOnOpen } from "./focus-cmdk-root";
import {
  FOOTER_SEPARATOR,
  FOOTER_TRIGGER,
  PICKER_COLLISION_PADDING,
  PICKER_COMMAND,
  PICKER_CONTENT,
  PICKER_GROUP,
  PICKER_LIST,
  PICKER_ROW,
  PICKER_ROW_CHECK,
  PICKER_ROW_DESCRIPTION,
  PICKER_ROW_TITLE,
  PICKER_SEPARATOR,
} from "./footer-trigger";

// Short description lines for each effort level. Verbs match
// PermissionModePicker's density (two-line rows).
const DEFAULT_EFFORT_DESCRIPTIONS: Record<string, string> = {
  none: "No extra reasoning",
  minimal: "Fastest, minimal reasoning",
  low: "Light reasoning",
  medium: "Balanced default",
  high: "Thorough reasoning",
  xhigh: "Extra-thorough reasoning",
  max: "Deepest reasoning",
  ultra: "Deepest reasoning with automatic task delegation",
  ultracode:
    "Extra-thorough reasoning plus standing multi-agent workflow orchestration",
  ultrathink:
    'Prepends "Ultrathink:" to your prompt for extra-thorough reasoning',
};

interface Props {
  /** Active chat model, or null when capabilities haven't loaded. */
  model: ChatModelInfo | null;
  /** Current selected effort from the thread slice, or null → default. */
  effortValue: string | null;
  /** Current selected context window from the thread slice, or null → default. */
  contextWindowValue: string | null;
  /** Canonical effort-label map from the provider capabilities. */
  labelMap: Record<string, string>;
  /** True when the composer's draft text contains "ultrathink" outside
   *  the canonical prefix — disables the effort section. */
  ultrathinkInBodyText: boolean;
  /** Premium service tier for models that advertise fast-mode support. */
  fastMode: boolean;
  /** Fires when the user picks an effort row. `"ultrathink"` carries
   *  the special meaning of "prepend to the prompt"; caller handles that. */
  onEffortChange: (nextValue: string) => void;
  /** Fires when the user picks a context-window row. */
  onContextWindowChange: (nextValue: string) => void;
  /** Fires when the user picks Standard or Fast service tier. */
  onFastModeChange: (fastMode: boolean) => void;
  disabled?: boolean;
  /** Render a leading hairline pipe. Lives inside the picker (not the
   *  footer) so the pipe disappears together with the control when the
   *  capability gate hides it — no orphaned separators. */
  withSeparator?: boolean;
  /** Narrow composer: keep the icon, drop the text label (the label stays
   *  in the tooltip / accessible name). Ignored when there is no icon. */
  iconOnly?: boolean;
}

function effortLabel(labelMap: Record<string, string>, id: string): string {
  return labelMap[id] ?? id;
}

// The provider's own catalog text wins when it ships one (Codex reports
// a blurb per effort level over `model/list`), so a level added upstream
// reads correctly without a frontend bump. The built-in map covers
// providers that report nothing.
function effortDescription(model: ChatModelInfo, id: string): string {
  return (
    model.effort_descriptions?.[id] ?? DEFAULT_EFFORT_DESCRIPTIONS[id] ?? ""
  );
}

/**
 * Combined Reasoning + Context Window + Service Tier picker.
 *
 * Replaces the separate `EffortPicker` and `ContextWindowPicker` that
 * used to render as two adjacent pills. Merged surface: one pill
 * shows "Effort · Context", one dropdown carries every model-level runtime
 * choice. Service tier lives here instead of taking a permanent footer slot.
 *
 * Null-slice fallback (Option C of the Stage C follow-up): when the
 * slice's effort or contextWindow is null, the picker resolves to the
 * model's default and renders THAT as the "current" value (label +
 * check). The user sees a consistent pill / dropdown state without
 * needing an explicit pick. The moment the user DOES pick something,
 * the slice value wins over the fallback.
 *
 * Render rules:
 *  - Hidden when `model` is null (capabilities unavailable).
 *  - Hidden when the model has no effort levels, ≤1 context-window options,
 *    AND no service-tier choice (e.g. Haiku 4.5 — nothing to pick).
 *  - The effort section only appears when the model has effort levels.
 *  - The context-window section only appears when the model has >1
 *    options.
 *  - The service-tier section only appears when the model supports fast mode.
 */
export function ReasoningPicker({
  model,
  effortValue,
  contextWindowValue,
  labelMap,
  ultrathinkInBodyText,
  fastMode,
  onEffortChange,
  onContextWindowChange,
  onFastModeChange,
  disabled,
  withSeparator,
  iconOnly = false,
}: Props) {
  const [open, setOpen] = useState(false);

  if (!model) return null;

  const effortLevels = [
    ...model.effort_levels,
    ...model.prompt_injected_effort_levels,
  ];
  const contextOptions = model.context_window_options;
  const hasEffortSection = effortLevels.length > 0;
  const hasContextSection = contextOptions.length > 1;
  const hasServiceTierSection = model.supports_fast_mode;

  // Haiku and other models without any configurable runtime choice: hide.
  if (!hasEffortSection && !hasContextSection && !hasServiceTierSection) {
    return null;
  }

  // Option C fallback — null slice values resolve to the model's
  // default. These are the values the pill reflects and the checkmarks
  // test against.
  const currentEffort =
    effortValue && effortLevels.includes(effortValue)
      ? effortValue
      : (model.default_effort ?? effortLevels[0] ?? null);

  const defaultContextWindow =
    contextOptions.find((o) => o.is_default)?.value ??
    contextOptions[0]?.value ??
    null;
  const currentContextWindow =
    contextWindowValue &&
    contextOptions.some((o) => o.value === contextWindowValue)
      ? contextWindowValue
      : defaultContextWindow;

  const effortLabelText = currentEffort
    ? effortLabel(labelMap, currentEffort)
    : null;
  const contextLabelText = currentContextWindow
    ? (contextOptions.find((o) => o.value === currentContextWindow)?.label ??
      currentContextWindow)
    : null;

  // Pill-label composition:
  //  - Both sections populated: "Effort · Context"
  //  - Only effort: just the effort label.
  //  - Only context: just the context label (edge case; no Claude
  //    model has this shape today but the branch keeps the picker
  //    future-proof).
  const reasoningLabel = (() => {
    if (effortLabelText && contextLabelText) {
      return `${effortLabelText} · ${contextLabelText}`;
    }
    return effortLabelText ?? contextLabelText ?? null;
  })();
  const triggerLabel = reasoningLabel ?? (fastMode ? "Fast" : "Standard");
  const triggerAriaLabel = reasoningLabel
    ? `Reasoning: ${reasoningLabel}; service tier: ${fastMode ? "Fast" : "Standard"}`
    : `Service tier: ${fastMode ? "Fast" : "Standard"}`;

  // Every row shares one recipe: title over a muted description (wraps
  // rather than truncating, so provider blurbs stay readable), check on
  // the right.
  const optionRow = (
    title: string,
    description: string,
    selected: boolean,
    isDefault: boolean,
  ) => (
    <>
      <div className="flex min-w-0 flex-1 flex-col gap-px">
        <span className={cn(PICKER_ROW_TITLE, "truncate")}>
          {title}
          {isDefault ? (
            <span className="ml-1.5 font-normal text-muted-foreground/60">
              (default)
            </span>
          ) : null}
        </span>
        {description ? (
          <span className={PICKER_ROW_DESCRIPTION}>{description}</span>
        ) : null}
      </div>
      <Check
        className={cn(
          PICKER_ROW_CHECK,
          selected ? "opacity-100" : "opacity-0",
        )}
      />
    </>
  );

  return (
    <>
      {withSeparator && <span aria-hidden className={FOOTER_SEPARATOR} />}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            disabled={disabled}
            aria-label={triggerAriaLabel}
            title={triggerAriaLabel}
            className={FOOTER_TRIGGER}
          >
            {fastMode ? (
              <Zap
                aria-hidden
                data-testid="fast-mode-indicator"
                className="size-3.5 fill-current text-foreground/80"
              />
            ) : reasoningLabel ? (
              <Brain className="size-3.5" />
            ) : null}
            {!(iconOnly && (fastMode || reasoningLabel)) && (
              <span className="max-w-[200px] truncate">{triggerLabel}</span>
            )}
            <ChevronDown className="size-3 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          className={PICKER_CONTENT}
          align="start"
          collisionPadding={PICKER_COLLISION_PADDING}
          onOpenAutoFocus={focusCmdkOnOpen}
        >
          <Command
            className={PICKER_COMMAND}
            // Start keyboard focus on the current choice, so the highlight
            // and the check mark never point at different rows on open.
            defaultValue={
              hasEffortSection && currentEffort
                ? `effort:${currentEffort}`
                : hasContextSection && currentContextWindow
                  ? `ctx:${currentContextWindow}`
                  : `service-tier:${fastMode ? "fast" : "standard"}`
            }
          >
            {hasEffortSection && ultrathinkInBodyText ? (
              <div className="px-2 pt-1.5 pb-1 text-label text-muted-foreground/80">
                Your prompt contains &quot;ultrathink&quot; in the text. Remove
                it to change effort.
              </div>
            ) : null}
            <CommandList className={PICKER_LIST}>
              <CommandEmpty>No reasoning options</CommandEmpty>

              {hasEffortSection && (
                <CommandGroup heading="Effort" className={PICKER_GROUP}>
                  {effortLevels.map((level) => (
                    <CommandItem
                      key={`effort-${level}`}
                      value={`effort:${level}`}
                      disabled={ultrathinkInBodyText}
                      onSelect={() => {
                        if (ultrathinkInBodyText) return;
                        onEffortChange(level);
                      }}
                      className={PICKER_ROW}
                      showCheckmark={false}
                    >
                      {optionRow(
                        effortLabel(labelMap, level),
                        effortDescription(model, level),
                        currentEffort === level,
                        level === model.default_effort,
                      )}
                    </CommandItem>
                  ))}
                </CommandGroup>
              )}

              {hasEffortSection &&
                (hasContextSection || hasServiceTierSection) && (
                  <CommandSeparator className={PICKER_SEPARATOR} />
                )}

              {hasContextSection && (
                <CommandGroup heading="Context window" className={PICKER_GROUP}>
                  {contextOptions.map((option) => (
                    <CommandItem
                      key={`ctx-${option.value}`}
                      value={`ctx:${option.value}`}
                      onSelect={() => onContextWindowChange(option.value)}
                      className={PICKER_ROW}
                      showCheckmark={false}
                    >
                      {optionRow(
                        option.label,
                        "",
                        currentContextWindow === option.value,
                        option.is_default,
                      )}
                    </CommandItem>
                  ))}
                </CommandGroup>
              )}

              {hasContextSection && hasServiceTierSection && (
                <CommandSeparator className={PICKER_SEPARATOR} />
              )}

              {hasServiceTierSection && (
                <CommandGroup heading="Service tier" className={PICKER_GROUP}>
                  <CommandItem
                    value="service-tier:standard"
                    onSelect={() => onFastModeChange(false)}
                    className={PICKER_ROW}
                    showCheckmark={false}
                  >
                    {optionRow(
                      "Standard",
                      "Normal speed and usage rate",
                      !fastMode,
                      true,
                    )}
                  </CommandItem>
                  <CommandItem
                    value="service-tier:fast"
                    onSelect={() => onFastModeChange(true)}
                    className={PICKER_ROW}
                    showCheckmark={false}
                  >
                    {optionRow(
                      "Fast",
                      "Faster output at a premium usage rate",
                      fastMode,
                      false,
                    )}
                  </CommandItem>
                </CommandGroup>
              )}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </>
  );
}
