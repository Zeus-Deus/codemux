import { useEffect, useMemo, useState } from "react";
import { ChevronDown, Star } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  pickerFavoriteKey,
  usePickerFavorites,
} from "@/stores/picker-favorites-store";
import {
  selectError,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  ProviderChatCapabilities,
} from "@/tauri/types";

import { ProviderLogo } from "../provider-logo";
import { focusCmdkRootOnOpen } from "./focus-cmdk-root";

/**
 * Step 12 Stage 4 — unified provider + model picker.
 *
 * Replaces the side-by-side `ProviderPicker + ModelPicker` pair with a
 * single 2-column popover: a provider rail on the left + a searchable
 * model list on the right. Federates OpenCode's per-`sub_provider`
 * model catalogue under one rail entry whose row subtitles read as
 * `OpenCode · {sub_provider}`. Codex is finally surfaced too — the
 * old picker's `ENABLE_PROVIDER_PICKER` flag gated it out of the UI
 * even though the backend has shipped 4 Codex models since Step 6-9.
 *
 * Locked decisions for v1:
 *
 * * No favorites (Stage 6).
 * * No keyboard shortcuts beyond cmdk's arrow + enter (Ctrl+1..9
 *   collides with workspace switching).
 * * Single instance per driver (Stage 5 — no per-instance accent
 *   badges yet; the `sub_provider` row subtitle is the only
 *   federation hint).
 * * Search collapses provider grouping into a flat result list across
 *   ALL providers; clearing the search snaps back to "show only the
 *   rail-selected provider's models".
 *
 * The entire surface is keyed off `provider-capabilities-store`, so
 * loading / error / connected states are live and refresh independently
 * per provider — an OpenCode harvest failure cannot block Claude/Codex
 * from rendering.
 */

const ALL_PROVIDERS: ReadonlyArray<{
  kind: AgentChatProviderKind;
  label: string;
}> = [
  { kind: "claude", label: "Claude" },
  { kind: "codex", label: "Codex" },
  { kind: "opencode", label: "OpenCode" },
];

interface Props {
  provider: AgentChatProviderKind;
  model: string | null;
  onProviderModelChange: (
    provider: AgentChatProviderKind,
    model: string,
  ) => void;
  disabled?: boolean;
}

interface ResolvedRow {
  model: ChatModelInfo;
  /** Driver kind the model came from. Drives the provider icon
   *  rendered next to the secondary subtitle line. */
  provider: AgentChatProviderKind;
}

export function MultiProviderModelPicker({
  provider,
  model,
  onProviderModelChange,
  disabled,
}: Props) {
  const [open, setOpen] = useState(false);
  const [railProvider, setRailProvider] =
    useState<AgentChatProviderKind>(provider);
  const [query, setQuery] = useState("");

  // Reset the rail + search to the active provider whenever the
  // popover opens. Lets the picker stay in sync with external
  // provider changes (session resume, settings restore) without
  // clobbering the user's manual rail clicks while the popover is
  // open.
  useEffect(() => {
    if (open) {
      setRailProvider(provider);
      setQuery("");
    }
  }, [open, provider]);

  const allCaps = useProviderCapabilities();
  const claudeCaps = allCaps.claude;
  const codexCaps = allCaps.codex;
  const opencodeCaps = allCaps.opencode;

  // Subscribe to the favorites array so toggling a star while the
  // popover is open re-sorts the visible rows immediately. Reading
  // the whole array (rather than calling `isFavorite` lazily) is
  // what tells zustand to re-render this component on every toggle.
  const favoritesArray = usePickerFavorites((s) => s.favorites);
  const favoritesSet = useMemo(
    () => new Set(favoritesArray),
    [favoritesArray],
  );

  // Provider model rows, organised so we can switch between
  // "single-provider list" and "all-providers flat list" without
  // re-iterating the whole capabilities store on every keystroke.
  const rowsByProvider = useMemo<
    Record<AgentChatProviderKind, ResolvedRow[]>
  >(() => {
    return {
      claude: rowsFromCaps("claude", claudeCaps),
      codex: rowsFromCaps("codex", codexCaps),
      opencode: rowsFromCaps("opencode", opencodeCaps),
    };
  }, [claudeCaps, codexCaps, opencodeCaps]);

  const visibleRows = useMemo<ResolvedRow[]>(() => {
    const trimmed = query.trim().toLowerCase();
    const base = !trimmed
      ? rowsByProvider[railProvider] ?? []
      : // Search collapses provider grouping — flatten all rows then
        // filter. Order: claude, codex, opencode (rail order).
        ALL_PROVIDERS.flatMap((p) => rowsByProvider[p.kind] ?? []).filter(
          (row) => matchesQuery(row, trimmed),
        );

    // Stage 6 — favorites bubble up to the top of the visible list
    // (across all surfaces: rail-only AND search-collapsed). Within
    // each group (favorited / not), preserve insertion order so the
    // capability harvest's row order isn't reshuffled. `slice()`
    // because `Array.sort` mutates and `rowsByProvider` is memoised.
    return base.slice().sort((a, b) => {
      const aFav = favoritesSet.has(pickerFavoriteKey(a.provider, a.model.id));
      const bFav = favoritesSet.has(pickerFavoriteKey(b.provider, b.model.id));
      if (aFav && !bFav) return -1;
      if (!aFav && bFav) return 1;
      return 0;
    });
  }, [query, railProvider, rowsByProvider, favoritesSet]);

  const selectedKey = `${provider}::${model ?? ""}`;
  const triggerLabel = useMemo(() => {
    const caps =
      provider === "claude"
        ? claudeCaps
        : provider === "codex"
        ? codexCaps
        : opencodeCaps;
    if (!caps && !model) return "Loading…";
    const found = caps?.models.find((m) => m.id === model);
    if (found) return found.label;
    if (model) return model;
    return "Select model";
  }, [provider, model, claudeCaps, codexCaps, opencodeCaps]);

  const triggerSubtitle = useMemo(() => {
    const caps =
      provider === "claude"
        ? claudeCaps
        : provider === "codex"
        ? codexCaps
        : opencodeCaps;
    return caps?.models.find((m) => m.id === model)?.sub_provider ?? null;
  }, [provider, model, claudeCaps, codexCaps, opencodeCaps]);

  return (
    <Popover open={open} onOpenChange={(v) => !disabled && setOpen(v)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          data-testid="multi-provider-model-picker-trigger"
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs",
            "text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
            "outline-none disabled:opacity-50",
          )}
        >
          <ProviderLogo provider={provider} className="h-3 w-3" />
          <span className="max-w-[180px] truncate">
            {triggerSubtitle ? (
              <>
                <span className="opacity-70">{triggerSubtitle}</span>
                <span aria-hidden className="mx-1 opacity-40">
                  ·
                </span>
                <span>{triggerLabel}</span>
              </>
            ) : (
              triggerLabel
            )}
          </span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        className="w-[600px] max-w-[calc(100vw-2rem)] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        <div className="grid grid-cols-[48px_1fr] h-[400px] overflow-hidden">
          <ProviderRail
            providers={ALL_PROVIDERS}
            selected={railProvider}
            getCount={(kind) => rowsByProvider[kind]?.length ?? 0}
            onSelect={(kind) => {
              setRailProvider(kind);
              setQuery("");
            }}
          />
          <div className="flex min-h-0 flex-col">
            <Command shouldFilter={false}>
              <CommandInput
                placeholder="Search models..."
                value={query}
                onValueChange={setQuery}
                className="h-9 text-xs"
              />
              <CommandList className="max-h-none flex-1 overflow-y-auto">
                <CommandEmpty>
                  <ModelListEmptyState
                    railProvider={railProvider}
                    query={query}
                    caps={
                      railProvider === "claude"
                        ? claudeCaps
                        : railProvider === "codex"
                        ? codexCaps
                        : opencodeCaps
                    }
                    error={selectError(allCaps, railProvider)}
                  />
                </CommandEmpty>
                {visibleRows.length === 0 ? (
                  <LoadingFallback
                    railProvider={railProvider}
                    caps={
                      railProvider === "claude"
                        ? claudeCaps
                        : railProvider === "codex"
                        ? codexCaps
                        : opencodeCaps
                    }
                    error={selectError(allCaps, railProvider)}
                    query={query}
                  />
                ) : (
                  visibleRows.map((row) => {
                    const isActive =
                      row.provider === provider && row.model.id === model;
                    const key = pickerFavoriteKey(row.provider, row.model.id);
                    return (
                      <ModelRow
                        key={key}
                        row={row}
                        isActive={isActive}
                        isFavorite={favoritesSet.has(key)}
                        onSelect={() => {
                          onProviderModelChange(row.provider, row.model.id);
                          setOpen(false);
                          setQuery("");
                        }}
                        onToggleFavorite={() =>
                          usePickerFavorites
                            .getState()
                            .toggle(row.provider, row.model.id)
                        }
                      />
                    );
                  })
                )}
              </CommandList>
            </Command>
          </div>
        </div>
        {/* Defensive: keep the cmdk-required outer Command wrapper out of
            the visible flow when no rows pass the filter — cmdk would
            otherwise refuse to render `<CommandEmpty>` because we
            disabled internal filtering with `shouldFilter={false}`.
            The `selectedKey` data attr is a smoke surface for tests. */}
        <span className="sr-only" data-active-key={selectedKey} />
      </PopoverContent>
    </Popover>
  );
}

function ProviderRail({
  providers,
  selected,
  getCount,
  onSelect,
}: {
  providers: ReadonlyArray<{ kind: AgentChatProviderKind; label: string }>;
  selected: AgentChatProviderKind;
  getCount: (kind: AgentChatProviderKind) => number;
  onSelect: (kind: AgentChatProviderKind) => void;
}) {
  return (
    <TooltipProvider delayDuration={200}>
      <div
        className="flex flex-col gap-1 border-r bg-muted/30 p-1"
        data-testid="multi-provider-rail"
      >
        {providers.map((p) => {
          const isSelected = selected === p.kind;
          const count = getCount(p.kind);
          return (
            <div key={p.kind} className="relative">
              {isSelected && (
                <span
                  className="pointer-events-none absolute -right-1 top-1/2 z-10 h-5 w-0.5 -translate-y-1/2 rounded-l-full bg-primary"
                  aria-hidden
                />
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => onSelect(p.kind)}
                    data-testid={`provider-rail-${p.kind}`}
                    data-selected={isSelected || undefined}
                    aria-label={p.label}
                    aria-pressed={isSelected}
                    className={cn(
                      "relative flex aspect-square w-full items-center justify-center rounded transition-colors",
                      "hover:bg-muted",
                      isSelected && "bg-background text-foreground shadow-sm",
                    )}
                  >
                    <ProviderLogo
                      provider={p.kind}
                      className="h-5 w-5 shrink-0"
                    />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="right" sideOffset={8}>
                  {p.label}
                  {count > 0 ? (
                    <span className="ml-2 text-muted-foreground">
                      {count}
                    </span>
                  ) : null}
                </TooltipContent>
              </Tooltip>
            </div>
          );
        })}
      </div>
    </TooltipProvider>
  );
}

function ModelRow({
  row,
  isActive,
  isFavorite,
  onSelect,
  onToggleFavorite,
}: {
  row: ResolvedRow;
  isActive: boolean;
  isFavorite: boolean;
  onSelect: () => void;
  onToggleFavorite: () => void;
}) {
  const { model, provider } = row;
  const driverLabel = providerDisplayLabel(provider);
  const subtitle = model.sub_provider
    ? `${driverLabel} · ${model.sub_provider}`
    : driverLabel;
  return (
    <CommandItem
      value={`${provider}::${model.id}`}
      onSelect={onSelect}
      data-testid="multi-provider-model-row"
      data-active={isActive || undefined}
      data-favorite={isFavorite || undefined}
      className={cn(
        "group flex w-full cursor-pointer items-start gap-2 px-3 py-2",
        "data-[active=true]:bg-accent/40",
      )}
    >
      <div className="min-w-0 flex-1 text-left">
        <div className="flex items-center gap-2 text-xs font-medium leading-snug">
          <span className="truncate">{model.label}</span>
        </div>
        <div className="mt-0.5 flex items-center gap-1 text-[11px] text-muted-foreground/70">
          <ProviderLogo
            provider={provider}
            className="h-2.5 w-2.5 shrink-0"
          />
          <span className="truncate">{subtitle}</span>
        </div>
      </div>
      {model.context_window_options[0]?.label ? (
        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground/60">
          {model.context_window_options[0].label}
        </span>
      ) : null}
      <button
        type="button"
        onClick={(e) => {
          // Don't let the click bubble into the row's `onSelect` —
          // starring is meant to be orthogonal to model selection,
          // and cmdk would otherwise close the popover on every star
          // toggle. The pointer-down handler is also stopped so cmdk's
          // synthetic-mouse-down → select pipeline doesn't fire.
          e.stopPropagation();
          onToggleFavorite();
        }}
        onPointerDown={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
        data-testid="model-row-favorite-toggle"
        data-favorite={isFavorite || undefined}
        aria-pressed={isFavorite}
        aria-label={
          isFavorite ? "Remove from favorites" : "Add to favorites"
        }
        className={cn(
          "shrink-0 rounded p-1 transition-opacity hover:bg-accent",
          isFavorite
            ? "text-amber-500 opacity-100"
            : "opacity-0 hover:opacity-100 group-hover:opacity-60 focus-visible:opacity-100",
        )}
      >
        <Star
          className="h-3.5 w-3.5"
          fill={isFavorite ? "currentColor" : "none"}
        />
      </button>
    </CommandItem>
  );
}

function ModelListEmptyState({
  railProvider,
  query,
  caps,
  error,
}: {
  railProvider: AgentChatProviderKind;
  query: string;
  caps: ProviderChatCapabilities | null;
  error: string | null;
}) {
  const trimmed = query.trim();
  if (trimmed.length > 0) {
    return (
      <div className="px-4 py-6 text-center text-xs text-muted-foreground">
        No models match{" "}
        <span className="font-medium text-foreground">"{trimmed}"</span>
      </div>
    );
  }
  if (railProvider === "opencode") {
    if (error === "opencode_not_installed") {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          <p className="font-medium text-foreground">
            OpenCode not detected on your system
          </p>
          <p className="mt-1">
            Install the <code className="rounded bg-muted px-1">opencode</code>{" "}
            CLI to access federated model providers.
          </p>
          <p className="mt-2">
            <a
              href="https://opencode.ai"
              target="_blank"
              rel="noreferrer noopener"
              className="text-foreground underline-offset-4 hover:underline"
            >
              opencode.ai
            </a>
          </p>
        </div>
      );
    }
    if (error) {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          OpenCode harvest failed:{" "}
          <span className="text-foreground">{error}</span>
        </div>
      );
    }
    if (caps && caps.models.length === 0) {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          <p className="font-medium text-foreground">No connected providers</p>
          <p className="mt-1">
            Run{" "}
            <code className="rounded bg-muted px-1">opencode auth login</code>{" "}
            to configure upstream credentials.
          </p>
        </div>
      );
    }
  }
  if (caps && caps.models.length === 0) {
    return (
      <div className="px-4 py-6 text-center text-xs text-muted-foreground">
        No models available
      </div>
    );
  }
  return null;
}

function LoadingFallback({
  railProvider,
  caps,
  error,
  query,
}: {
  railProvider: AgentChatProviderKind;
  caps: ProviderChatCapabilities | null;
  error: string | null;
  query: string;
}) {
  // Suppress the skeleton when an empty-state already covers the
  // intent (search-no-match / OpenCode-not-installed / etc.). We do
  // that by deferring to `ModelListEmptyState` for those cases and
  // only render skeletons for the genuine "still loading" branch.
  const trimmed = query.trim();
  const hasEmptyState =
    trimmed.length > 0 ||
    error !== null ||
    (caps !== null && caps.models.length === 0);
  if (hasEmptyState) {
    return null;
  }
  // Genuine "still loading": caps is still null with no error.
  return (
    <div
      className="space-y-2 px-3 py-2"
      data-testid={`multi-provider-loading-${railProvider}`}
    >
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="flex items-center gap-2">
          <div className="min-w-0 flex-1">
            <Skeleton className="h-3 w-24" />
            <Skeleton className="mt-1 h-2 w-16" />
          </div>
        </div>
      ))}
    </div>
  );
}

function rowsFromCaps(
  provider: AgentChatProviderKind,
  caps: ProviderChatCapabilities | null,
): ResolvedRow[] {
  if (!caps) return [];
  return caps.models.map((model) => ({ provider, model }));
}

function matchesQuery(row: ResolvedRow, query: string): boolean {
  const haystack: string[] = [
    row.model.label.toLowerCase(),
    row.model.id.toLowerCase(),
    providerDisplayLabel(row.provider).toLowerCase(),
  ];
  if (row.model.sub_provider) {
    haystack.push(row.model.sub_provider.toLowerCase());
  }
  return haystack.some((s) => s.includes(query));
}

function providerDisplayLabel(provider: AgentChatProviderKind): string {
  switch (provider) {
    case "claude":
      return "Claude";
    case "codex":
      return "Codex";
    case "opencode":
      return "OpenCode";
  }
}
