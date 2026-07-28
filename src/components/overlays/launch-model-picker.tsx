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
import { focusCmdkOnOpen } from "@/components/chat/pickers/focus-cmdk-root";
import { cn } from "@/lib/utils";
import {
  MODEL_SEARCH_THRESHOLD,
  type LaunchModel,
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
  /** Set the model, or `null` to reset to the agent default. Picking
   *  `null` (the "Default" row) also clears reasoning/context upstream. */
  onModelChange: (model: string | null) => void;
  /** Extra classes for the trigger button. Lets a non-composer host (e.g.
   *  the preset editor, a settings form) override the default rounded-full
   *  pill to match surrounding form fields. */
  triggerClassName?: string;
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
 * Launch-time model picker for the New Workspace dialog.
 *
 * Models only — reasoning and context are attributes of the chosen
 * model and live in the sibling `LaunchReasoningPicker` pill (mirroring
 * the chat composer's `ModelPicker` + `ReasoningPicker` split). Picking
 * a model closes the popover; the reasoning/context pill then appears
 * next to this one.
 *
 * Renders a compact pill next to the agent picker. The popover adapts
 * to list size: a short list (Claude, Codex, Gemini) is a flat
 * glanceable list; a long federated list (OpenCode) gains a search
 * box, per-`subProvider` group headers, and star-to-favorite rows.
 *
 * Every row ends in one fixed-width trailing slot at the right edge —
 * a favorite star on model rows, a checkmark on the Default row — so the
 * two line up in a single column. The selected model row is marked by a
 * background tint (the star keeps the slot). "Default" leaves the choice
 * to the agent and emits no flag.
 */
export function LaunchModelPicker({
  providerKind,
  models,
  loading,
  selectedModel,
  onModelChange,
  triggerClassName,
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

  /** A simple right-edge row: label + a checkmark in the shared
   *  trailing slot. Used for the "Default" row. */
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
        // Close on pick: reasoning/context now live in a sibling pill,
        // so a model choice completes this popover's job.
        onSelect={() => {
          onModelChange(model.id);
          setOpen(false);
        }}
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
                  ? "fill-current text-status-working"
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
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full border border-border bg-muted/50 px-2.5 py-1 text-[11px] text-muted-foreground transition-colors outline-none hover:bg-muted hover:text-foreground",
            triggerClassName,
          )}
        >
          <span className="max-w-[150px] truncate">{selectedModelLabel}</span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40 ml-auto" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[300px] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkOnOpen}
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
            {/* One master "Default" — resets to the agent's own default
                (and clears reasoning/context upstream via
                `onModelChange(null)`). Checked when no model is
                overridden. */}
            <CommandGroup>
              {renderChoiceRow(
                "default-all",
                "default:__all__",
                "Default",
                selectedModel === null,
                () => {
                  onModelChange(null);
                  setOpen(false);
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
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
