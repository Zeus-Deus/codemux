import { useMemo, useState } from "react";
import { Check, ChevronDown, Star } from "lucide-react";

import {
  Command,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { focusCmdkRootOnOpen } from "@/components/chat/pickers/focus-cmdk-root";
import { cn } from "@/lib/utils";
import {
  MODEL_SEARCH_THRESHOLD,
  type LaunchModel,
  type ReasoningOption,
} from "@/lib/launch-models";
import {
  pickerFavoriteKey,
  usePickerFavorites,
} from "@/stores/picker-favorites-store";
import type { AgentChatProviderKind } from "@/tauri/types";

interface Props {
  /** Chat-capability provider kind, or `null` for Gemini (no favorites
   *  store key — Gemini's list is short enough not to need them). */
  providerKind: AgentChatProviderKind | null;
  /** Models offered for the selected agent. */
  models: LaunchModel[];
  /** True while the capability harvest is still in flight. */
  loading?: boolean;
  /** The user's explicit model override, or `null` for the agent default. */
  selectedModel: string | null;
  /** The user's explicit reasoning override, or `null` for the default. */
  selectedReasoning: string | null;
  /** Reasoning levels the agent's CLI accepts. Empty → no reasoning row. */
  reasoningOptions: ReasoningOption[];
  /** Context-window options for the selected model (Claude only).
   *  Empty → no context row. */
  contextOptions: ReasoningOption[];
  selectedContext: string | null;
  onModelChange: (model: string | null) => void;
  onReasoningChange: (reasoning: string | null) => void;
  onContextChange: (context: string | null) => void;
}

/** Scrollbar styling shared with the chat picker — overrides cmdk's
 *  `no-scrollbar` default so a 400-model list has a real, draggable
 *  scrollbar instead of a silently-clipped list. */
const SCROLLBAR =
  "[&]:[scrollbar-width:thin] [&::-webkit-scrollbar]:w-2 " +
  "[&::-webkit-scrollbar-thumb]:rounded-full " +
  "[&::-webkit-scrollbar-thumb]:bg-muted-foreground/30 " +
  "hover:[&::-webkit-scrollbar-thumb]:bg-muted-foreground/50";

/** A `data-slot="command-shortcut"` element makes shadcn's `CommandItem`
 *  suppress the checkmark it auto-appends (`command.tsx`). The picker
 *  renders its own trailing slot, so the auto-check must be out of the
 *  layout entirely — otherwise it claims the right edge and the star
 *  can never line up with it. `hidden` keeps it out of the flex flow. */
function SuppressAutoCheck() {
  return <span data-slot="command-shortcut" className="hidden" aria-hidden />;
}

function matchesQuery(model: LaunchModel, query: string): boolean {
  if (!query) return true;
  const q = query.toLowerCase();
  return (
    model.id.toLowerCase().includes(q) ||
    model.label.toLowerCase().includes(q) ||
    (model.subProvider ?? "").toLowerCase().includes(q)
  );
}

/**
 * Launch-time model + reasoning picker for the New Workspace dialog.
 *
 * Renders a compact pill next to the agent picker. The popover adapts
 * to list size: a short list (Claude, Codex, Gemini) is a flat
 * glanceable list; a long federated list (OpenCode) gains a search
 * box, per-`subProvider` group headers, and star-to-favorite rows.
 *
 * Every row ends in one fixed-width trailing slot at the right edge —
 * a favorite star on model rows, a checkmark on the Default / reasoning
 * rows — so the two line up in a single column. The selected model row
 * is marked by a background tint (the star keeps the slot). "Default"
 * leaves the choice to the agent and emits no flag.
 */
export function LaunchModelPicker({
  providerKind,
  models,
  loading,
  selectedModel,
  selectedReasoning,
  reasoningOptions,
  contextOptions,
  selectedContext,
  onModelChange,
  onReasoningChange,
  onContextChange,
}: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  const favoritesArray = usePickerFavorites((s) => s.favorites);
  const toggleFavorite = usePickerFavorites((s) => s.toggle);

  const favoritesSet = useMemo(
    () => new Set(favoritesArray),
    [favoritesArray],
  );
  const showFavorites = providerKind !== null;
  const showSearch = models.length > MODEL_SEARCH_THRESHOLD;

  const filtered = useMemo(
    () => models.filter((m) => matchesQuery(m, query)),
    [models, query],
  );

  // Favorited models bubble into their own group; the rest stay in
  // their normal (sub-provider) grouping. A model is never rendered
  // twice — cmdk needs unique item values.
  const { favorites, grouped } = useMemo(() => {
    const favs: LaunchModel[] = [];
    const rest: LaunchModel[] = [];
    for (const m of filtered) {
      const isFav =
        providerKind !== null &&
        favoritesSet.has(pickerFavoriteKey(providerKind, m.id));
      (isFav ? favs : rest).push(m);
    }
    const groups = new Map<string, LaunchModel[]>();
    for (const m of rest) {
      const key = m.subProvider ?? "";
      const bucket = groups.get(key);
      if (bucket) bucket.push(m);
      else groups.set(key, [m]);
    }
    return { favorites: favs, grouped: [...groups.entries()] };
  }, [filtered, favoritesSet, providerKind]);

  const selectedModelLabel = useMemo(() => {
    if (!selectedModel) return "Default";
    return models.find((m) => m.id === selectedModel)?.label ?? selectedModel;
  }, [models, selectedModel]);

  // Compact pill suffix — reasoning and context shown after the model
  // name as "· High · 1M" when set.
  const pillExtras = useMemo(() => {
    const parts: string[] = [];
    if (selectedReasoning) {
      parts.push(
        reasoningOptions.find((r) => r.value === selectedReasoning)?.label ??
          selectedReasoning,
      );
    }
    if (selectedContext) {
      parts.push(
        contextOptions.find((c) => c.value === selectedContext)?.label ??
          selectedContext,
      );
    }
    return parts;
  }, [selectedReasoning, selectedContext, reasoningOptions, contextOptions]);

  /** A simple right-edge row: label + a checkmark in the shared
   *  trailing slot. Used for "Default" and the reasoning options. */
  const renderChoiceRow = (
    key: string,
    value: string,
    label: string,
    isSelected: boolean,
    onPick: () => void,
  ) => (
    <CommandItem
      key={key}
      value={value}
      data-checked={isSelected ? "true" : undefined}
      onSelect={onPick}
      className={cn(
        "gap-2 text-xs",
        isSelected && "bg-accent/70 data-[selected=true]:bg-accent/70",
      )}
    >
      <SuppressAutoCheck />
      <span
        className={cn(
          "flex-1 truncate",
          isSelected ? "font-medium text-foreground" : "text-foreground",
        )}
      >
        {label}
      </span>
      <span className="flex size-5 shrink-0 items-center justify-center">
        {isSelected ? <Check className="size-3.5 text-foreground" /> : null}
      </span>
    </CommandItem>
  );

  const renderModelRow = (model: LaunchModel) => {
    const isSelected = selectedModel === model.id;
    const isFav =
      providerKind !== null &&
      favoritesSet.has(pickerFavoriteKey(providerKind, model.id));
    return (
      <CommandItem
        key={model.id}
        value={`model:${model.id}`}
        data-checked={isSelected ? "true" : undefined}
        // Keep the popover open after a model pick so the Reasoning and
        // Context Window sections below stay reachable in the same
        // interaction — a model pick should not dismiss the picker.
        onSelect={() => onModelChange(model.id)}
        className={cn(
          "gap-2 text-xs",
          isSelected && "bg-accent/70 data-[selected=true]:bg-accent/70",
        )}
      >
        <SuppressAutoCheck />
        <div className="flex min-w-0 flex-1 flex-col">
          <span
            className={cn(
              "truncate",
              isSelected ? "font-medium text-foreground" : "text-foreground",
            )}
          >
            {model.label}
          </span>
          {model.subProvider ? (
            <span className="truncate text-[10px] text-muted-foreground/70">
              {model.subProvider}
            </span>
          ) : null}
        </div>
        {/* Trailing slot — identical size-5 box on every row, so the
            star here lines up with the checkmark on the rows above. */}
        {showFavorites ? (
          <button
            type="button"
            aria-label={isFav ? "Unfavorite model" : "Favorite model"}
            className="flex size-5 shrink-0 items-center justify-center rounded transition-colors hover:bg-foreground/10"
            onClick={(e) => {
              e.stopPropagation();
              if (providerKind) toggleFavorite(providerKind, model.id);
            }}
            onPointerDown={(e) => e.stopPropagation()}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <Star
              className={cn(
                "size-3.5",
                isFav
                  ? "fill-current text-amber-500"
                  : "text-muted-foreground/40",
              )}
            />
          </button>
        ) : (
          <span className="flex size-5 shrink-0 items-center justify-center">
            {isSelected ? <Check className="size-3.5 text-foreground" /> : null}
          </span>
        )}
      </CommandItem>
    );
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Select model"
          className="inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors outline-none hover:bg-muted hover:text-foreground"
        >
          <span className="max-w-[150px] truncate">{selectedModelLabel}</span>
          {pillExtras.length > 0 ? (
            <span className="opacity-50">· {pillExtras.join(" · ")}</span>
          ) : null}
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[300px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <Command shouldFilter={false}>
          {showSearch ? (
            <CommandInput
              value={query}
              onValueChange={setQuery}
              placeholder={`Search ${models.length} models…`}
            />
          ) : null}
          {/* `onWheel` stops the wheel event before it reaches the
              enclosing modal Dialog's scroll-lock (`react-remove-scroll`),
              which would otherwise swallow it and freeze the list.
              Same fix `project-picker.tsx` uses for its in-dialog list. */}
          <CommandList
            className={cn("max-h-[340px] overflow-y-auto", SCROLLBAR)}
            onWheel={(e) => e.stopPropagation()}
          >
            {/* One master "Default" — resets model, reasoning, and
                context together. Checked only when nothing is
                overridden; the sections below carry no "Default" rows
                of their own, so there is exactly one in the popover. */}
            <CommandGroup>
              {renderChoiceRow(
                "default-all",
                "default:__all__",
                "Default",
                selectedModel === null &&
                  selectedReasoning === null &&
                  selectedContext === null,
                () => {
                  onModelChange(null);
                  onReasoningChange(null);
                  onContextChange(null);
                },
              )}
            </CommandGroup>
            <CommandSeparator />

            {loading && models.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">
                Loading models…
              </div>
            ) : null}

            {!loading && models.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">
                No models available
              </div>
            ) : null}

            {models.length > 0 && filtered.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">
                No models match &quot;{query}&quot;
              </div>
            ) : null}

            {favorites.length > 0 ? (
              <CommandGroup heading="Favorites">
                {favorites.map(renderModelRow)}
              </CommandGroup>
            ) : null}

            {grouped.map(([subProvider, list]) => (
              <CommandGroup
                key={subProvider || "models"}
                heading={
                  subProvider ||
                  (favorites.length > 0 ? "Models" : undefined)
                }
              >
                {list.map(renderModelRow)}
              </CommandGroup>
            ))}

            {reasoningOptions.length > 0 ? (
              <>
                <CommandSeparator />
                <CommandGroup heading="Reasoning">
                  {reasoningOptions.map((opt) =>
                    renderChoiceRow(
                      `reasoning-${opt.value}`,
                      `reasoning:${opt.value}`,
                      opt.label,
                      selectedReasoning === opt.value,
                      () => onReasoningChange(opt.value),
                    ),
                  )}
                </CommandGroup>
              </>
            ) : null}

            {contextOptions.length > 0 ? (
              <>
                <CommandSeparator />
                <CommandGroup heading="Context Window">
                  {contextOptions.map((opt) =>
                    renderChoiceRow(
                      `context-${opt.value}`,
                      `context:${opt.value}`,
                      opt.label,
                      selectedContext === opt.value,
                      () => onContextChange(opt.value),
                    ),
                  )}
                </CommandGroup>
              </>
            ) : null}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
