import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { CaseSensitive, ChevronDown, ChevronUp, Regex, X } from "lucide-react";
import type { ISearchOptions, SearchAddon } from "@xterm/addon-search";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type TerminalSearch = Pick<
  SearchAddon,
  "findNext" | "findPrevious" | "clearDecorations" | "onDidChangeResults"
>;

interface Props {
  search: TerminalSearch;
  /** Bumped each time the find shortcut fires, so a second press re-selects
   *  the query instead of doing nothing. */
  focusToken: number;
  onClose: () => void;
}

// Outlines are DOM styles, so they can follow the theme token. The overview
// ruler fields are required by the addon but unused: the terminal has no ruler.
const DECORATIONS: NonNullable<ISearchOptions["decorations"]> = {
  matchBorder: "color-mix(in oklab, var(--accent-ember) 55%, transparent)",
  activeMatchBorder: "var(--accent-ember)",
  matchOverviewRuler: "transparent",
  activeMatchColorOverviewRuler: "transparent",
};

/** Find-in-scrollback bar anchored to the top-right of a terminal pane. */
export function TerminalFindBar({ search, focusToken, onClose }: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [regex, setRegex] = useState(false);
  const [invalid, setInvalid] = useState(false);
  const [results, setResults] = useState<{ index: number; count: number } | null>(null);

  useEffect(() => {
    const input = inputRef.current;
    input?.focus();
    input?.select();
  }, [focusToken]);

  useEffect(() => {
    const sub = search.onDidChangeResults(({ resultIndex, resultCount }) =>
      setResults({ index: resultIndex, count: resultCount }),
    );
    return () => sub.dispose();
  }, [search]);

  const run = useCallback(
    (direction: "next" | "previous", incremental: boolean) => {
      if (!query) {
        search.clearDecorations();
        setResults(null);
        setInvalid(false);
        return;
      }
      const options: ISearchOptions = {
        caseSensitive,
        regex,
        incremental,
        decorations: DECORATIONS,
      };
      try {
        if (direction === "next") search.findNext(query, options);
        else search.findPrevious(query, options);
        setInvalid(false);
      } catch {
        // An unfinished regular expression while typing.
        search.clearDecorations();
        setResults(null);
        setInvalid(true);
      }
    },
    [caseSensitive, query, regex, search],
  );

  // Re-run as the query or its options change, extending the current match
  // rather than jumping past it.
  useEffect(() => {
    run("next", true);
  }, [run]);

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      run(event.shiftKey ? "previous" : "next", false);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
    }
  };

  const status = invalid
    ? "Invalid pattern"
    : !query || !results
      ? ""
      : results.count === 0
        ? "No results"
        : `${results.index >= 0 ? results.index + 1 : "?"}/${results.count}`;

  return (
    <div
      role="search"
      aria-label="Find in terminal"
      className="absolute right-3 top-2 z-10 flex items-center gap-1 rounded-md border border-border bg-popover py-1 pl-2 pr-1 text-popover-foreground shadow-md has-[input:focus]:ring-2 has-[input:focus]:ring-ring/60 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-top-1 motion-safe:duration-150"
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Find"
        aria-label="Find in terminal"
        aria-invalid={invalid || undefined}
        spellCheck={false}
        className="h-6 w-40 min-w-0 bg-transparent text-body-sm outline-none placeholder:text-muted-foreground aria-invalid:text-destructive"
      />
      <span
        aria-live="polite"
        className={cn(
          "min-w-12 text-right text-label tabular-nums text-muted-foreground",
          invalid && "text-destructive",
        )}
      >
        {status}
      </span>
      <Button
        variant="ghost"
        size="icon-xs"
        title="Match case"
        aria-label="Match case"
        aria-pressed={caseSensitive}
        onClick={() => setCaseSensitive((value) => !value)}
        className={cn(caseSensitive && "bg-surface-3 text-foreground")}
      >
        <CaseSensitive className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        title="Use regular expression"
        aria-label="Use regular expression"
        aria-pressed={regex}
        onClick={() => setRegex((value) => !value)}
        className={cn(regex && "bg-surface-3 text-foreground")}
      >
        <Regex className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        title="Previous match (Shift+Enter)"
        aria-label="Previous match"
        disabled={!query || invalid}
        onClick={() => run("previous", false)}
      >
        <ChevronUp className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        title="Next match (Enter)"
        aria-label="Next match"
        disabled={!query || invalid}
        onClick={() => run("next", false)}
      >
        <ChevronDown className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        title="Close (Escape)"
        aria-label="Close find"
        onClick={onClose}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}
