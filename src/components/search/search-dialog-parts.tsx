import type { Ref } from "react";
import { Loader2 } from "lucide-react";
import { Input } from "@/components/ui/input";
import { KeyHint } from "@/components/ui/key-hint";
import { cn } from "@/lib/utils";

/**
 * The query field both search dialogs share. The pending-search spinner
 * lives inside the input's right edge so the result list below never moves
 * while the user types.
 */
export function SearchQueryInput({
  inputRef,
  value,
  onChange,
  placeholder,
  loading,
}: {
  inputRef: Ref<HTMLInputElement>;
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  loading: boolean;
}) {
  return (
    <div className="relative">
      <Input
        ref={inputRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-9 pr-8 text-body"
      />
      {loading && (
        <Loader2
          aria-label="Searching"
          className="pointer-events-none absolute top-1/2 right-2.5 size-3.5 -translate-y-1/2 text-muted-foreground motion-safe:animate-spin"
        />
      )}
    </div>
  );
}

/** Classes for a result list that is about to be replaced: still readable,
 *  visibly stale, never shifted. */
export function staleListClass(stale: boolean) {
  return cn(
    "motion-safe:transition-opacity motion-safe:duration-150",
    stale && "opacity-60",
  );
}

/** The key hints both dialogs end with, in the palette's footer chrome. */
export function SearchFooter({ hints }: { hints: Array<{ keys: string; label: string }> }) {
  return (
    <div className="flex h-[34px] flex-none items-center gap-3.5 border-t border-hairline bg-surface-1 px-3.5">
      {hints.map((hint) => (
        <KeyHint key={hint.keys} keys={hint.keys} label={hint.label} />
      ))}
    </div>
  );
}

/** Closes a result list that stopped at the backend's limit, so a full page
 *  of results is never mistaken for every match. */
export function ResultCapNotice({ limit }: { limit: number }) {
  return (
    <p className="px-2 pt-2 pb-1 text-center text-caption text-muted-foreground tabular-nums">
      Showing the first {limit} · refine your query to narrow it down
    </p>
  );
}

/** `path` relative to the workspace root, for display. */
export function relativeToRoot(root: string, path: string): string {
  const prefix = root.endsWith("/") ? root : `${root}/`;
  return root && path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

/** Where `query` first occurs in `text`, ignoring case, as [start, end). */
export function findMatchRange(text: string, query: string): [number, number] | null {
  const needle = query.trim().toLowerCase();
  if (!needle) return null;
  const start = text.toLowerCase().indexOf(needle);
  return start < 0 ? null : [start, start + needle.length];
}

/** Renders `text` with `[start, end)` emphasized; plain text when the range
 *  is missing or out of bounds. */
export function MatchHighlight({
  text,
  range,
  className,
}: {
  text: string;
  range: [number, number] | null;
  className: string;
}) {
  if (!range) return <>{text}</>;
  const [start, end] = range;
  if (start < 0 || end <= start || start >= text.length) return <>{text}</>;
  return (
    <>
      {text.slice(0, start)}
      <mark className={className}>{text.slice(start, end)}</mark>
      {text.slice(end)}
    </>
  );
}
