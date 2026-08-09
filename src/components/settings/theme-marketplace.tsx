import { useEffect, useRef, useState } from "react";
import { Loader2, Search } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  fetchMarketplaceThemes,
  searchMarketplaceThemes,
  type MarketplaceTheme,
  type MarketplaceThemeVariant,
} from "@/tauri/commands";

const DEBOUNCE_MS = 350;

/**
 * Find a VS Code theme by name instead of by file.
 *
 * The import flow's worst step was never the parsing — it was getting the
 * JSON in the first place: find the extension, find the `.vsix`, unzip it,
 * find which of the files inside is the variant you meant. Codemux does all
 * of that; you type "tokyo night".
 *
 * Selecting a result fetches its variants and hands the chosen one's raw
 * JSONC to the same parser a paste goes through, so there is exactly one
 * import path and the Marketplace is just a nicer way to reach it.
 */
export function ThemeMarketplacePanel({
  onPick,
}: {
  /** Receives the raw colour-theme JSONC, exactly as a paste would. */
  onPick: (content: string, label: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MarketplaceTheme[] | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [variants, setVariants] = useState<MarketplaceThemeVariant[] | null>(null);
  const [loadingVariants, setLoadingVariants] = useState(false);

  // One in-flight search wins: a slow response for "toky" must not overwrite
  // the results for "tokyo night".
  const generation = useRef(0);

  useEffect(() => {
    const needle = query.trim();
    if (needle.length < 2) {
      setResults(null);
      setError(null);
      setSearching(false);
      return;
    }
    const mine = ++generation.current;
    setSearching(true);
    const timer = window.setTimeout(() => {
      searchMarketplaceThemes(needle)
        .then((found) => {
          if (generation.current !== mine) return;
          // A host without the command (the browser dev mock, an older
          // desktop build) resolves with nothing rather than rejecting.
          // Rendering an empty list there reads as "no such theme", which is
          // a lie about a search that never ran.
          if (!Array.isArray(found)) {
            setResults(null);
            setError("Marketplace search isn't available on this host.");
            return;
          }
          setResults(found);
          setError(null);
        })
        .catch((cause: unknown) => {
          if (generation.current !== mine) return;
          setResults(null);
          setError(cause instanceof Error ? cause.message : String(cause));
        })
        .finally(() => {
          if (generation.current === mine) setSearching(false);
        });
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [query]);

  const openExtension = (theme: MarketplaceTheme) => {
    setExpanded(theme.extension_id);
    setVariants(null);
    setLoadingVariants(true);
    setError(null);
    fetchMarketplaceThemes(theme.vsix_url)
      .then((found) => {
        setVariants(found);
        // A single dark variant is not a choice — take it.
        if (found.length === 1) onPick(found[0]!.content, found[0]!.label);
      })
      .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setLoadingVariants(false));
  };

  return (
    <div className="flex flex-col gap-[7px]">
      <span className="text-[11.5px] font-semibold text-muted-foreground">
        Search the Marketplace
      </span>

      <label className="flex h-[34px] items-center gap-2 rounded-[9px] border border-border bg-muted/30 px-2.5">
        <Search className="size-3 flex-none text-muted-foreground" />
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="tokyo night"
          aria-label="Search the VS Code Marketplace"
          className="min-w-0 flex-1 bg-transparent text-[12px] text-foreground outline-none placeholder:text-muted-foreground/60"
        />
        {searching && <Loader2 className="size-3 flex-none animate-spin text-muted-foreground" />}
      </label>

      {error && (
        <p className="rounded-lg border border-destructive/25 bg-destructive/10 px-2.5 py-2 text-[11px] leading-relaxed text-destructive">
          {error}
        </p>
      )}

      {results?.length === 0 && !searching && (
        <p className="text-[10.5px] text-muted-foreground/70">
          Nothing on the Marketplace matches{" "}
          <span className="font-mono text-foreground">{query.trim()}</span>.
        </p>
      )}

      {results && results.length > 0 && (
        <div className="flex flex-col gap-1">
          {results.map((theme) => {
            const open = expanded === theme.extension_id;
            return (
              <div key={theme.extension_id} className="flex flex-col gap-1">
                <button
                  type="button"
                  onClick={() => openExtension(theme)}
                  className={cn(
                    "flex items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left transition-colors",
                    open ? "bg-muted" : "hover:bg-muted/60",
                  )}
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[11.5px] font-semibold text-foreground">
                      {theme.display_name}
                    </span>
                    <span className="block truncate text-[10px] text-muted-foreground">
                      {theme.publisher} · {formatInstalls(theme.install_count)} installs
                    </span>
                  </span>
                  {open && loadingVariants && (
                    <Loader2 className="size-3 flex-none animate-spin text-muted-foreground" />
                  )}
                  {open && variants && (
                    <span className="flex-none font-mono text-[10px] text-muted-foreground">
                      {variants.length} variant{variants.length === 1 ? "" : "s"}
                    </span>
                  )}
                </button>

                {/* Variants only matter when there is a decision to make. */}
                {open && variants && variants.length > 1 && (
                  <div className="flex flex-col gap-px pl-2.5">
                    {variants.map((variant) => (
                      <button
                        key={variant.label}
                        type="button"
                        onClick={() => onPick(variant.content, variant.label)}
                        className="flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-[11.5px] text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
                      >
                        {variant.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      <span className="text-[10.5px] leading-relaxed text-muted-foreground/70">
        Dark variants only — Codemux themes are dark for now.
      </span>
    </div>
  );
}

function formatInstalls(count: number): string {
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1_000) return `${Math.round(count / 1_000)}K`;
  return `${count}`;
}
