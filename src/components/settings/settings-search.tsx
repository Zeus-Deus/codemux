import { useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Search, X } from "lucide-react";

import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import {
  searchSettings,
  splitHighlight,
  type SettingsSearchResult,
} from "@/lib/settings-search";

/** How long a found row stays lit after search lands on it. Matches the
 *  `cm-settings-flash` animation in globals.css. */
const FLASH_MS = 1_600;
/** Room left above or below a nav row scrolled into view. */
const NAV_SCROLL_MARGIN = 12;

/** Scroll `scroller` the least distance that shows `row`. Unlike
 *  `scrollIntoView`, this never moves the page's other scroll containers. */
function keepInView(scroller: HTMLElement, row: HTMLElement) {
  const box = scroller.getBoundingClientRect();
  const rect = row.getBoundingClientRect();
  if (rect.top < box.top) {
    scroller.scrollTop -= box.top - rect.top + NAV_SCROLL_MARGIN;
  } else if (rect.bottom > box.bottom) {
    scroller.scrollTop += rect.bottom - box.bottom + NAV_SCROLL_MARGIN;
  }
}

/**
 * The search field at the top of the Settings nav. While it has a query, it
 * replaces the nav (`children`) with matching rows grouped by page; Enter or a
 * click hands the result to `onNavigate` and clears the query, so the nav comes
 * back with the destination page selected.
 */
export function SettingsNavSearch({
  agentChatEnabled,
  activeSection,
  inputRef,
  onNavigate,
  children,
}: {
  agentChatEnabled: boolean;
  /** The open page; its nav row is kept in view when it changes. */
  activeSection: string;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onNavigate: (result: SettingsSearchResult) => void;
  children: React.ReactNode;
}) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const listId = useId();
  const scrollerRef = useRef<HTMLDivElement>(null);

  const groups = useMemo(
    () => searchSettings(query, agentChatEnabled),
    [query, agentChatEnabled],
  );
  const results = useMemo(() => groups.flatMap((g) => g.results), [groups]);
  const searching = query.trim() !== "";
  const active = Math.min(activeIndex, Math.max(results.length - 1, 0));

  // A search result or a palette jump can open a page whose nav row sits below
  // the fold. Once the nav is back, scroll only the nav so that row shows.
  useLayoutEffect(() => {
    const scroller = scrollerRef.current;
    if (searching || !scroller) return;
    const row = scroller.querySelector<HTMLElement>('[aria-current="page"]');
    if (row) keepInView(scroller, row);
  }, [activeSection, searching]);

  const go = (result: SettingsSearchResult | undefined) => {
    if (!result) return;
    setQuery("");
    setActiveIndex(0);
    onNavigate(result);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (results.length === 0) return;
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActiveIndex((active + step + results.length) % results.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      go(results[active]);
    } else if (e.key === "Escape" && searching) {
      // Escape closes Settings globally; with a query it only clears it.
      e.stopPropagation();
      setQuery("");
      setActiveIndex(0);
    }
  };

  let index = -1;
  return (
    <>
      <div className="shrink-0 px-3 pb-4">
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground/70" />
          <input
            ref={inputRef}
            type="text"
            role="combobox"
            aria-label="Search settings"
            aria-expanded={searching}
            aria-controls={listId}
            aria-activedescendant={searching && results.length > 0 ? `${listId}-${active}` : undefined}
            aria-autocomplete="list"
            placeholder="Search settings"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActiveIndex(0);
            }}
            onKeyDown={onKeyDown}
            spellCheck={false}
            className="h-8 w-full rounded-md border border-hairline-strong bg-surface-1 pr-8 pl-8 text-body-sm text-foreground transition-colors duration-100 outline-none placeholder:text-muted-foreground/70 hover:bg-surface-2 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/60"
          />
          {searching ? (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => {
                setQuery("");
                inputRef.current?.focus();
              }}
              className="absolute top-1/2 right-1.5 flex size-5 -translate-y-1/2 items-center justify-center rounded-sm text-muted-foreground transition-colors duration-100 hover:bg-surface-2 hover:text-foreground"
            >
              <X className="size-3" />
            </button>
          ) : (
            <kbd className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded-sm border border-hairline-strong px-1 font-mono text-micro text-muted-foreground/70">
              /
            </kbd>
          )}
        </div>
      </div>

      {/* The field stays put; only the list below it scrolls. */}
      <div ref={scrollerRef} className="min-h-0 flex-1 overflow-y-auto pb-4 thin-scrollbar">
        {!searching ? (
          children
        ) : groups.length === 0 ? (
          <p className="px-4 text-body-sm text-muted-foreground" role="status">
            No settings match “{query.trim()}”.
          </p>
        ) : (
          <div id={listId} role="listbox" aria-label="Settings search results" className="space-y-4">
            {groups.map((group) => (
              <div key={group.section} role="group" aria-label={group.sectionLabel}>
                <Eyebrow className="px-4 pb-1.5">{group.sectionLabel}</Eyebrow>
                <div className="space-y-px px-3">
                  {group.results.map((result) => {
                    index += 1;
                    const selected = index === active;
                    const optionIndex = index;
                    return (
                      <div
                        key={`${result.section}:${result.anchor ?? ""}`}
                        id={`${listId}-${optionIndex}`}
                        role="option"
                        aria-selected={selected}
                        // Keep focus in the field while clicking a result.
                        onMouseDown={(e) => e.preventDefault()}
                        onMouseMove={() => setActiveIndex(optionIndex)}
                        onClick={() => go(result)}
                        className={cn(
                          "flex min-h-8 cursor-pointer items-center gap-2 rounded-lg px-2.5 py-1.5 text-body transition-colors duration-150",
                          selected ? "bg-surface-3 text-foreground" : "text-muted-foreground/90",
                        )}
                      >
                        {/* Long labels wrap rather than truncate: the nav is
                            narrow and the label is the whole answer. */}
                        <span className="min-w-0 flex-1 leading-snug break-words">
                          <Highlighted text={result.label} query={query} />
                          {result.matchedKeyword && (
                            <span className="block text-label text-muted-foreground/70">
                              Matches <Highlighted text={result.matchedKeyword} query={query} />
                            </span>
                          )}
                        </span>
                        {result.anchor === null && (
                          <span className="shrink-0 self-start pt-px text-label text-muted-foreground/70">
                            Page
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}

function Highlighted({ text, query }: { text: string; query: string }) {
  return (
    <>
      {splitHighlight(text, query).map((part, i) =>
        i % 2 === 1 ? (
          <mark key={i} className="bg-transparent font-semibold text-foreground">
            {part}
          </mark>
        ) : (
          part
        ),
      )}
    </>
  );
}

/**
 * Scroll the row labelled `label` inside `root` into view and light it
 * briefly. Rows are found by their visible text, so every page's rows are
 * reachable without each one registering an anchor; `data-settings-row`
 * widens the lit area to the whole row where a page marks one. Returns false
 * when the page has no such row (yet), so the caller can fall back to the top.
 */
export function revealSettingsAnchor(root: HTMLElement, label: string): boolean {
  const target = Array.from(root.querySelectorAll<HTMLElement>("*")).find(
    (el) => el.childElementCount === 0 && el.textContent?.trim() === label,
  );
  if (!target) return false;
  const row = target.closest<HTMLElement>("[data-settings-row]") ?? target.parentElement ?? target;
  const reduceMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  row.scrollIntoView?.({ block: "center", behavior: reduceMotion ? "auto" : "smooth" });
  row.setAttribute("data-settings-flash", "");
  window.setTimeout(() => row.removeAttribute("data-settings-flash"), FLASH_MS);
  return true;
}
