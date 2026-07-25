import { useEffect, useMemo, useRef, useState } from "react";
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
import { FOOTER_TRIGGER } from "./footer-trigger";

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

/**
 * Filter `ALL_PROVIDERS` by an optional allowed-set. When `allowed` is
 * undefined every provider is returned (chat behavior). When `allowed`
 * is supplied (settings: commits is claude-only because the commit-
 * message backend in `src-tauri/src/ai.rs:generate_commit_message`
 * hardcodes the claude CLI), only those entries appear in the rail and
 * in the cross-provider search results. Kept as a pure helper so we
 * don't have to thread the predicate through every internal function.
 */
function filterProviders(
  allowed: ReadonlyArray<AgentChatProviderKind> | undefined,
): typeof ALL_PROVIDERS {
  if (!allowed || allowed.length === 0) return ALL_PROVIDERS;
  const allowedSet = new Set(allowed);
  return ALL_PROVIDERS.filter((p) => allowedSet.has(p.kind));
}

/** Discriminated shape for the prefixed error strings the capability
 *  harvest emits. The OpenCode adapter ships bare tokens
 *  (`opencode_not_installed`); the Codex adapter ships
 *  `codex_<kind>: <hint>` so each failure mode can render distinct UX
 *  without re-querying the backend. Anything we can't classify lands
 *  in `unknown` and falls through to the generic "Unavailable" copy. */
type ProviderErrorKind =
  | "not_installed"
  | "not_authenticated"
  | "harvest_failed"
  | "unknown";

interface ParsedProviderError {
  kind: ProviderErrorKind;
  /** Backend-supplied hint (the bit after the colon). Null for the
   *  bare-token OpenCode case where no hint was attached. */
  detail: string | null;
}

function parseProviderError(error: string | null): ParsedProviderError | null {
  if (!error) return null;
  const colonIdx = error.indexOf(":");
  const head = colonIdx >= 0 ? error.slice(0, colonIdx) : error;
  const detail =
    colonIdx >= 0 ? error.slice(colonIdx + 1).trim() || null : null;
  switch (head) {
    case "codex_not_installed":
    case "opencode_not_installed":
      return { kind: "not_installed", detail };
    case "codex_not_authenticated":
    case "opencode_not_authenticated":
      return { kind: "not_authenticated", detail };
    case "codex_harvest_failed":
    case "opencode_harvest_failed":
      return { kind: "harvest_failed", detail };
    default:
      return { kind: "unknown", detail: error };
  }
}

function providerErrorTooltipLabel(parsed: ParsedProviderError): string {
  switch (parsed.kind) {
    case "not_installed":
      return "Not installed";
    case "not_authenticated":
      return "Not signed in";
    case "harvest_failed":
    case "unknown":
      return "Unavailable";
  }
}

interface Props {
  provider: AgentChatProviderKind;
  model: string | null;
  onProviderModelChange: (
    provider: AgentChatProviderKind,
    model: string,
  ) => void;
  disabled?: boolean;
  /**
   * Optional allowlist of providers. When omitted (chat usage), every
   * provider is shown. When set (settings → commit messages), only
   * those providers render in the rail and only their models appear in
   * cross-provider search. Used by surfaces whose backend supports a
   * subset of the providers — e.g. commit messages is claude-only on
   * the Rust side, so passing `["claude"]` here keeps the picker
   * visually consistent with the resolver picker while preventing the
   * user from choosing a model the backend will reject.
   */
  allowedProviders?: ReadonlyArray<AgentChatProviderKind>;
  /** Imperative open request — increments each time the composer's
   *  `/model` slash command fires. `0` / `undefined` means "no
   *  request yet"; any increment pops the picker open. */
  openSignal?: number;
}

interface ResolvedRow {
  model: ChatModelInfo;
  /** Driver kind the model came from. Drives the provider icon
   *  rendered next to the secondary subtitle line. */
  provider: AgentChatProviderKind;
}

/** Pseudo-provider for the "favorites" rail entry. Renders all
 *  favorited rows across drivers in one list. Kept as a string union
 *  with `AgentChatProviderKind` so the rest of the rail logic stays
 *  uniform. */
type RailKey = AgentChatProviderKind | "favorites";

/** Platform check for the jump-shortcut modifier and its chip label
 *  ("⌘1" on macOS, "Ctrl+1" elsewhere). */
const IS_MAC =
  typeof navigator !== "undefined" && /mac/i.test(navigator.platform);
const JUMP_MOD_LABEL = IS_MAC ? "⌘" : "Ctrl+";

export function MultiProviderModelPicker({
  provider,
  model,
  onProviderModelChange,
  disabled,
  allowedProviders,
  openSignal,
}: Props) {
  const [open, setOpen] = useState(false);
  const [railKey, setRailKey] = useState<RailKey>(provider);
  const [query, setQuery] = useState("");

  // Consume the open signal exactly once per increment. Tracking the
  // last-seen value in a ref (instead of gating on `openSignal &&
  // !disabled`) prevents the picker from spontaneously reopening every
  // time `disabled` flips back to false — e.g. on session start/restart
  // — after `/model` has been used once in the pane's lifetime.
  const lastSignal = useRef(openSignal);
  useEffect(() => {
    if (openSignal !== lastSignal.current) {
      lastSignal.current = openSignal;
      if (!disabled) setOpen(true);
    }
  }, [openSignal, disabled]);
  // Filter the providers list once per render. Memoized via the
  // identity of `allowedProviders` so consumers passing a stable
  // array reference (the common case) don't churn the rail.
  const visibleProviders = useMemo(
    () => filterProviders(allowedProviders),
    [allowedProviders],
  );

  // Reset the rail + search to the active provider whenever the
  // popover opens. Lets the picker stay in sync with external
  // provider changes (session resume, settings restore) without
  // clobbering the user's manual rail clicks while the popover is
  // open.
  useEffect(() => {
    if (open) {
      setRailKey(provider);
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

  // The favorites pseudo-tab in the rail must be gated on favorites
  // FOR THE CURRENTLY VISIBLE PROVIDERS, not on the unfiltered total.
  // Without this filter, a picker with `allowedProviders=["claude"]`
  // and only Codex/OpenCode favorites would show an empty favorites
  // tab — which looks like the favorites silently disappeared. Each
  // favorites key is `${provider}::${id}`, so we filter by the
  // provider prefix.
  const visibleFavoritesCount = useMemo(() => {
    if (!allowedProviders || allowedProviders.length === 0) {
      return favoritesArray.length;
    }
    const allowedSet = new Set(allowedProviders);
    return favoritesArray.filter((key) => {
      const sep = key.indexOf("::");
      if (sep < 0) return false;
      return allowedSet.has(key.slice(0, sep) as AgentChatProviderKind);
    }).length;
  }, [favoritesArray, allowedProviders]);

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
    // Cross-provider search only ever flattens the providers we've been
    // allowed to show — passing an `allowedProviders` allowlist must
    // keep restricted models out of search results too, not just the
    // rail.
    const allRows = visibleProviders.flatMap(
      (p) => rowsByProvider[p.kind] ?? [],
    );
    const base = trimmed
      ? // Search collapses both rail entries and provider grouping —
        // flatten all rows then filter. Order: claude, codex, opencode
        // (rail order). Favorites filter is implicit (favorited rows
        // still appear if they match the query).
        allRows.filter((row) => matchesQuery(row, trimmed))
      : railKey === "favorites"
        ? // Favorites tab: every starred row across drivers, in
          // rail-order (claude → codex → opencode) so the layout
          // stays predictable as the user adds/removes stars.
          allRows.filter((row) =>
            favoritesSet.has(pickerFavoriteKey(row.provider, row.model.id)),
          )
        : rowsByProvider[railKey] ?? [];

    // Two-level sort:
    //   1. Favorites bubble to the very top of every list (rail-only,
    //      cross-provider search, and the favorites tab itself —
    //      where it's a no-op since every row is starred).
    //   2. Within the non-favorites group, free-tier OpenCode models
    //      bubble above paid ones. Some upstreams (OpenRouter, Venice,
    //      Vercel, …) rotate which models are free month-to-month, so
    //      surfacing them up top makes the rotation discoverable
    //      without forcing the user to scroll the long federated list.
    // Within each tier we preserve the upstream insertion order so the
    // capability harvest's order isn't reshuffled.
    return base.slice().sort((a, b) => {
      const aFav = favoritesSet.has(pickerFavoriteKey(a.provider, a.model.id));
      const bFav = favoritesSet.has(pickerFavoriteKey(b.provider, b.model.id));
      if (aFav && !bFav) return -1;
      if (!aFav && bFav) return 1;
      if (a.model.is_free && !b.model.is_free) return -1;
      if (!a.model.is_free && b.model.is_free) return 1;
      return 0;
    });
  }, [query, railKey, rowsByProvider, favoritesSet, visibleProviders]);

  // mod+1..9 jump shortcuts — bound to the first nine rows of the
  // CURRENT list (search filter, rail tab, and favorites sort all
  // respected, since `visibleRows` is the exact array the list
  // renders). Window capture-phase listener so the shortcut fires
  // even while the search input holds focus; registered only while
  // the popover is open. Selecting closes the picker, same as a click.
  const jumpRowsRef = useRef<ResolvedRow[]>([]);
  jumpRowsRef.current = visibleRows.slice(0, 9);
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.repeat) return;
      const mod = IS_MAC
        ? event.metaKey && !event.ctrlKey
        : event.ctrlKey && !event.metaKey;
      if (!mod || event.shiftKey || event.altKey) return;
      if (!/^[1-9]$/.test(event.key)) return;
      const row = jumpRowsRef.current[Number(event.key) - 1];
      if (!row) return;
      event.preventDefault();
      event.stopPropagation();
      onProviderModelChange(row.provider, row.model.id);
      setOpen(false);
      setQuery("");
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [open, onProviderModelChange]);

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

  // The active model's resolved-version blurb, surfaced only in the
  // trigger's tooltip so the pill itself stays compact. Combines the
  // visible label with the description (e.g.
  // "Opus · Opus 4.8 with 1M context · Best for everyday, complex tasks").
  const triggerTitle = useMemo(() => {
    const caps =
      provider === "claude"
        ? claudeCaps
        : provider === "codex"
        ? codexCaps
        : opencodeCaps;
    const description =
      caps?.models.find((m) => m.id === model)?.description ?? null;
    return description ? `${triggerLabel} · ${description}` : triggerLabel;
  }, [provider, model, claudeCaps, codexCaps, opencodeCaps, triggerLabel]);

  return (
    <Popover open={open} onOpenChange={(v) => !disabled && setOpen(v)}>
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          data-testid="multi-provider-model-picker-trigger"
          title={triggerTitle}
          // Composer footer control: quiet ghost text (shared
          // FOOTER_TRIGGER recipe), the leading ProviderLogo standing
          // in for a tinted dot. Hairline pipes between footer
          // controls — not per-pill borders — carry the separation.
          className={FOOTER_TRIGGER}
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
        // Compact popup: 400px wide (48px provider rail + fluid list),
        // 384px tall — a scannable column instead of a wide panel.
        className="w-[400px] max-w-[calc(100vw-2rem)] p-0"
        align="start"
        onOpenAutoFocus={focusCmdkRootOnOpen}
      >
        {/* `minmax(0,1fr)` (not bare `1fr`): grid items default to
            min-width auto, so a long unbreakable subtitle line would
            otherwise force the list column past the popup width and
            clip the rows against the rail. */}
        <div className="grid grid-cols-[48px_minmax(0,1fr)] h-[384px] overflow-hidden">
          <ProviderRail
            providers={visibleProviders}
            selected={railKey}
            favoritesCount={visibleFavoritesCount}
            getCount={(kind) => rowsByProvider[kind]?.length ?? 0}
            getError={(kind) => selectError(allCaps, kind)}
            onSelect={(next) => {
              setRailKey(next);
              setQuery("");
            }}
          />
          <div className="flex min-h-0 min-w-0 flex-col">
            <Command shouldFilter={false}>
              <CommandInput
                placeholder="Search models..."
                value={query}
                onValueChange={setQuery}
                className="h-9 text-xs"
              />
              {/* Override cmdk's default `no-scrollbar max-h-72` so the
                  list fills the popover and shows a real scrollbar
                  when models overflow (Claude has 5, Codex 4, but
                  OpenCode federates ~150 connected-upstream models on
                  a fully-configured machine — without a scrollbar
                  half the list is unreachable). */}
              <CommandList className="max-h-none flex-1 overflow-y-auto [&]:[scrollbar-width:thin] [&::-webkit-scrollbar]:w-2 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-muted-foreground/30 hover:[&::-webkit-scrollbar-thumb]:bg-muted-foreground/50">
                <CommandEmpty>
                  <ModelListEmptyState
                    railKey={railKey}
                    query={query}
                    caps={capsForRail(
                      railKey,
                      claudeCaps,
                      codexCaps,
                      opencodeCaps,
                    )}
                    error={errorForRail(railKey, allCaps)}
                  />
                </CommandEmpty>
                {visibleRows.length === 0 ? (
                  <LoadingFallback
                    railKey={railKey}
                    caps={capsForRail(
                      railKey,
                      claudeCaps,
                      codexCaps,
                      opencodeCaps,
                    )}
                    error={errorForRail(railKey, allCaps)}
                    query={query}
                  />
                ) : (
                  visibleRows.map((row, index) => {
                    const isActive =
                      row.provider === provider && row.model.id === model;
                    const key = pickerFavoriteKey(row.provider, row.model.id);
                    return (
                      <ModelRow
                        key={key}
                        row={row}
                        isActive={isActive}
                        isFavorite={favoritesSet.has(key)}
                        jumpLabel={
                          index < 9 ? `${JUMP_MOD_LABEL}${index + 1}` : null
                        }
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
  favoritesCount,
  getCount,
  getError,
  onSelect,
}: {
  providers: ReadonlyArray<{ kind: AgentChatProviderKind; label: string }>;
  selected: RailKey;
  favoritesCount: number;
  getCount: (kind: AgentChatProviderKind) => number;
  getError: (kind: AgentChatProviderKind) => string | null;
  onSelect: (next: RailKey) => void;
}) {
  return (
    <TooltipProvider delayDuration={200}>
      <div
        className="flex flex-col gap-1 border-r bg-muted/30 p-1"
        data-testid="multi-provider-rail"
      >
        {/* Favorites pseudo-tab — sits above the driver entries with
            a hairline separator. Click filters the right column to
            every starred row across drivers. Hidden when the user has
            zero favorites so we don't surface an empty tab on a
            fresh install. */}
        {favoritesCount > 0 ? (
          <>
            <div className="relative">
              {selected === "favorites" && (
                <span
                  className="pointer-events-none absolute -right-1 top-1/2 z-10 h-5 w-0.5 -translate-y-1/2 rounded-l-full bg-primary"
                  aria-hidden
                />
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <button
                    type="button"
                    onClick={() => onSelect("favorites")}
                    data-testid="provider-rail-favorites"
                    data-selected={selected === "favorites" || undefined}
                    aria-label="Favorites"
                    aria-pressed={selected === "favorites"}
                    className={cn(
                      "relative flex aspect-square w-full items-center justify-center rounded transition-colors",
                      "hover:bg-muted",
                      selected === "favorites" &&
                        "bg-background text-foreground shadow-sm",
                    )}
                  >
                    <Star
                      className="h-5 w-5 shrink-0 text-status-working"
                      fill="currentColor"
                    />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="right" sideOffset={8}>
                  Favorites
                  <span className="ml-2 text-muted-foreground">
                    {favoritesCount}
                  </span>
                </TooltipContent>
              </Tooltip>
            </div>
            <div className="my-1 border-t border-border/60" aria-hidden />
          </>
        ) : null}
        {providers.map((p) => {
          const isSelected = selected === p.kind;
          const count = getCount(p.kind);
          const error = getError(p.kind);
          // A driver is "unavailable" when its capabilities harvest
          // failed. Both OpenCode (bare token) and Codex (prefixed
          // string with hint) can hit this; Claude ships a static
          // fallback bundle that never errors. The icon stays
          // clickable so the empty state in the right column can
          // render the install / sign-in hint; the dim opacity is
          // just a rail-level heads-up.
          const parsedError = parseProviderError(error);
          const isUnavailable = parsedError !== null;
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
                    data-unavailable={isUnavailable || undefined}
                    aria-label={p.label}
                    aria-pressed={isSelected}
                    className={cn(
                      "relative flex aspect-square w-full items-center justify-center rounded transition-colors",
                      "hover:bg-muted",
                      isSelected && "bg-background text-foreground shadow-sm",
                      isUnavailable &&
                        !isSelected &&
                        "opacity-40 hover:opacity-70",
                    )}
                  >
                    <ProviderLogo
                      provider={p.kind}
                      className="h-5 w-5 shrink-0"
                    />
                  </button>
                </TooltipTrigger>
                <TooltipContent side="right" sideOffset={8}>
                  <div className="flex items-center gap-2">
                    <span>{p.label}</span>
                    {!isUnavailable && count > 0 ? (
                      <span className="text-muted-foreground">{count}</span>
                    ) : null}
                  </div>
                  {parsedError ? (
                    <div className="mt-0.5 text-muted-foreground">
                      {providerErrorTooltipLabel(parsedError)}
                    </div>
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

/** Resolve the right-column caps bundle for a rail key. The
 *  `favorites` rail is cross-driver, so it has no single
 *  capabilities source — return `null` and let the empty-state path
 *  short-circuit when there are zero favorites. */
function capsForRail(
  rail: RailKey,
  claudeCaps: ProviderChatCapabilities | null,
  codexCaps: ProviderChatCapabilities | null,
  opencodeCaps: ProviderChatCapabilities | null,
): ProviderChatCapabilities | null {
  switch (rail) {
    case "claude":
      return claudeCaps;
    case "codex":
      return codexCaps;
    case "opencode":
      return opencodeCaps;
    case "favorites":
      return null;
  }
}

function errorForRail(
  rail: RailKey,
  state: Parameters<typeof selectError>[0],
): string | null {
  if (rail === "favorites") return null;
  return selectError(state, rail);
}

function ModelRow({
  row,
  isActive,
  isFavorite,
  jumpLabel,
  onSelect,
  onToggleFavorite,
}: {
  row: ResolvedRow;
  isActive: boolean;
  isFavorite: boolean;
  /** "⌘N" / "Ctrl+N" chip for the first nine rows of the current
   *  list, `null` beyond them. */
  jumpLabel?: string | null;
  onSelect: () => void;
  onToggleFavorite: () => void;
}) {
  const { model, provider } = row;
  const driverLabel = providerDisplayLabel(provider);
  // Base subtitle: driver label, plus the federated `sub_provider`
  // hint for OpenCode rows. When the backend supplies a resolved
  // `description` (e.g. Claude alias rows carry
  // "Opus 4.8 with 1M context · Best for everyday, complex tasks"),
  // append it after another dot so the row reads
  // "Claude · Opus 4.8 with 1M context · Best…" — single line,
  // truncated, with the full text in the `title` tooltip.
  const subtitleBase = model.sub_provider
    ? `${driverLabel} · ${model.sub_provider}`
    : driverLabel;
  const subtitle = model.description
    ? `${subtitleBase} · ${model.description}`
    : subtitleBase;
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
        <div className="text-xs font-medium leading-snug">
          <span className="truncate">{model.label}</span>
        </div>
        <div className="mt-0.5 flex items-center gap-1 text-[11px] text-muted-foreground/70">
          <ProviderLogo
            provider={provider}
            className="h-2.5 w-2.5 shrink-0"
          />
          <span className="truncate" title={subtitle}>
            {subtitle}
          </span>
        </div>
      </div>
      {jumpLabel ? (
        <kbd
          aria-hidden
          data-testid="model-row-jump-chip"
          className="pointer-events-none mt-0.5 inline-flex h-4 shrink-0 select-none items-center self-start rounded-sm bg-muted px-1.5 font-sans text-[10px] font-medium text-muted-foreground"
        >
          {jumpLabel}
        </kbd>
      ) : null}
      {model.is_free ? (
        <span
          data-testid="model-row-free-badge"
          className="mt-0.5 shrink-0 self-start rounded border border-status-open/35 bg-status-open/15 px-1 py-px text-[9px] font-bold uppercase leading-none tracking-wide text-status-open dark:border-status-open/30 dark:bg-status-open/12 dark:text-status-open"
          aria-label="Free model"
        >
          Free
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
        // The base CommandItem appends an invisible trailing CheckIcon
        // (16px + flex gap) to every row unless the row carries its own
        // trailing UI, detected via this slot — without it every row
        // drags a ~24px phantom gutter on its right edge.
        data-slot="command-shortcut"
        data-favorite={isFavorite || undefined}
        aria-pressed={isFavorite}
        aria-label={
          isFavorite ? "Remove from favorites" : "Add to favorites"
        }
        className={cn(
          // Always visible (not hover-revealed): a dim outline star on
          // every row keeps the favoriting affordance discoverable and
          // the row layout stable.
          "shrink-0 rounded p-1 transition-colors hover:bg-accent",
          isFavorite
            ? "text-status-working"
            : "text-muted-foreground/40 hover:text-foreground focus-visible:text-foreground",
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
  railKey,
  query,
  caps,
  error,
}: {
  railKey: RailKey;
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
  if (railKey === "favorites") {
    // The favorites rail is only rendered when `favoritesCount > 0`,
    // so an empty state here means the user just removed their last
    // favorite while the popover was open. Show a friendly cue.
    return (
      <div className="px-4 py-6 text-center text-xs text-muted-foreground">
        <p className="font-medium text-foreground">No favorites yet</p>
        <p className="mt-1">
          Click the star on any model row to favorite it.
        </p>
      </div>
    );
  }
  if (railKey === "opencode") {
    const parsed = parseProviderError(error);
    if (parsed?.kind === "not_installed") {
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
  if (railKey === "codex") {
    const parsed = parseProviderError(error);
    if (parsed?.kind === "not_installed") {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          <p className="font-medium text-foreground">
            Codex not detected on your system
          </p>
          <p className="mt-1">
            Install the <code className="rounded bg-muted px-1">codex</code>{" "}
            CLI and ensure it is on your PATH.
          </p>
          <p className="mt-2">
            <a
              href="https://github.com/openai/codex"
              target="_blank"
              rel="noreferrer noopener"
              className="text-foreground underline-offset-4 hover:underline"
            >
              github.com/openai/codex
            </a>
          </p>
        </div>
      );
    }
    if (parsed?.kind === "not_authenticated") {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          <p className="font-medium text-foreground">
            Codex is not signed in
          </p>
          <p className="mt-1">
            Run{" "}
            <code className="rounded bg-muted px-1">codex login</code>{" "}
            in a terminal and try again.
          </p>
        </div>
      );
    }
    if (parsed?.kind === "harvest_failed" || parsed?.kind === "unknown") {
      return (
        <div className="px-4 py-6 text-center text-xs text-muted-foreground">
          Codex harvest failed:{" "}
          <span className="text-foreground">
            {parsed.detail ?? error ?? ""}
          </span>
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
  railKey,
  caps,
  error,
  query,
}: {
  railKey: RailKey;
  caps: ProviderChatCapabilities | null;
  error: string | null;
  query: string;
}) {
  // Suppress the skeleton when an empty-state already covers the
  // intent (search-no-match / OpenCode-not-installed / etc.). We do
  // that by deferring to `ModelListEmptyState` for those cases and
  // only render skeletons for the genuine "still loading" branch.
  const trimmed = query.trim();
  // The favorites pseudo-rail has no caps to load — defer to the
  // empty-state path entirely (which renders "No favorites yet" or
  // the search-no-match message above).
  if (railKey === "favorites") {
    return null;
  }
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
      data-testid={`multi-provider-loading-${railKey}`}
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
