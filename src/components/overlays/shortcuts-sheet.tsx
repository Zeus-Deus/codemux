import { useMemo, useRef, useState } from "react";
import { Keyboard, Search, X } from "lucide-react";

import {
  DIALOG_CRISP_POSITION,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { formatKeyCombo, useShortcutLabel } from "@/components/ui/menu-chrome";
import {
  useResolvedKeybinds,
  type ResolvedEntry,
} from "@/hooks/use-resolved-keybinds";
import {
  CATEGORY_LABELS,
  KEYBIND_CATEGORIES,
  type KeybindCategory,
} from "@/lib/keybind-registry";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

/** Guards against an accidental reload. Real bindings, but nothing anyone
 *  looks up, so they stay on the Settings page only. */
const HIDDEN_IDS = new Set(["blockReload", "blockHardReload", "blockF5Reload"]);

export interface ShortcutRow {
  id: string;
  label: string;
  keys: string;
}

export interface ShortcutGroup {
  category: KeybindCategory;
  label: string;
  rows: ShortcutRow[];
}

/**
 * Fold a numbered series ("Switch to tab 1" … "Switch to tab 9") into one row
 * when its bindings still follow the series, so nine near-identical rows read
 * as "Switch to tab 1–9 · Ctrl+1–9". A series the user has rebound unevenly
 * stays expanded, since one row could no longer describe it.
 */
function foldSeries(entries: ResolvedEntry[]): ShortcutRow[] {
  const rows: ShortcutRow[] = [];
  const seen = new Set<string>();
  for (const entry of entries) {
    if (seen.has(entry.id)) continue;
    const match = entry.id.match(/^(.*\D)1$/);
    if (match) {
      const series = entries.filter((e) =>
        new RegExp(`^${match[1]}[1-9]$`).test(e.id),
      );
      const prefix = entry.activeKeys.slice(0, -1);
      const regular =
        series.length === 9 &&
        series.every((e) => e.activeKeys === `${prefix}${e.id.slice(-1)}`);
      if (regular) {
        series.forEach((e) => seen.add(e.id));
        rows.push({
          id: entry.id,
          label: entry.label.replace(/1$/, "1–9"),
          keys: `${formatKeyCombo(entry.activeKeys).slice(0, -1)}1–9`,
        });
        continue;
      }
    }
    seen.add(entry.id);
    rows.push({
      id: entry.id,
      label: entry.label,
      keys: formatKeyCombo(entry.activeKeys),
    });
  }
  return rows;
}

/** The cheat sheet's groups, in registry order, filtered by `query` against
 *  each row's label and keys. Unbound actions are left out. */
export function buildShortcutGroups(
  keybindMap: Map<string, ResolvedEntry>,
  query: string,
): ShortcutGroup[] {
  const needle = query.trim().toLowerCase();
  const groups: ShortcutGroup[] = [];
  for (const category of KEYBIND_CATEGORIES) {
    const entries = [...keybindMap.values()].filter(
      (e) => e.category === category && e.activeKeys && !HIDDEN_IDS.has(e.id),
    );
    const rows = foldSeries(entries).filter(
      (row) =>
        !needle ||
        row.label.toLowerCase().includes(needle) ||
        row.keys.toLowerCase().includes(needle),
    );
    if (rows.length > 0) {
      groups.push({ category, label: CATEGORY_LABELS[category], rows });
    }
  }
  return groups;
}

/**
 * Read-only reference for every binding, layered over whatever is open so
 * checking a shortcut never means leaving the work. Rebinding stays on
 * Settings ▸ Shortcuts, one click away. Mounted only while open (see
 * `App`), so the filter starts empty each time.
 */
export function ShortcutsSheet() {
  const setOpen = useUIStore((s) => s.setShowShortcutsSheet);
  const { keybindMap } = useResolvedKeybinds();
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  // Escape reaches the sheet only through `closeOverlay`, which is
  // rebindable, so the hint names whatever actually closes it.
  const closeKeys = useShortcutLabel("closeOverlay");
  const groups = useMemo(
    () => buildShortcutGroups(keybindMap, query),
    [keybindMap, query],
  );

  const customize = () => {
    setOpen(false);
    useUIStore.getState().setShowSettings(true, "shortcuts");
  };

  return (
    <Dialog open onOpenChange={setOpen}>
      <DialogContent
        showCloseButton={false}
        // Escape belongs to the app's close ladder (`closeOverlay`), which
        // closes exactly one layer per press. Dismissing here as well would
        // also close the Settings page underneath.
        onEscapeKeyDown={(event) => event.preventDefault()}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          inputRef.current?.focus();
        }}
        data-testid="shortcuts-sheet"
        // mobile.css centres every dialog's `top` on the mobile shell and
        // relies on the default -50% translate, which the crisp position
        // drops. Pin the sheet near the top instead, as the command palette
        // does, and keep mobile.css's dialog padding off its edge-to-edge chrome.
        className={cn(
          DIALOG_CRISP_POSITION,
          "flex max-h-[calc(100vh-12rem)] w-[640px] max-w-[calc(100vw-32px)] flex-col gap-0 overflow-hidden border border-border p-0 ring-0 sm:max-w-[640px] in-[[data-mobile]]:top-[calc(var(--mobile-top,0px)+12px)]! in-[[data-mobile]]:p-0!",
        )}
      >
        <div className="flex items-center gap-2.5 border-b border-hairline py-3 pr-3 pl-4">
          <Keyboard className="size-4 shrink-0 text-muted-foreground" />
          <span className="flex min-w-0 flex-1 flex-col gap-0.5">
            <DialogTitle className="text-body-lg leading-tight font-semibold">
              Keyboard shortcuts
            </DialogTitle>
            <DialogDescription className="sr-only">
              Every keyboard shortcut, grouped by area.
            </DialogDescription>
          </span>
          <DialogClose asChild>
            <Button variant="ghost" size="icon-sm" aria-label="Close keyboard shortcuts">
              <X className="size-3.5" />
            </Button>
          </DialogClose>
        </div>

        <label className="flex h-10 shrink-0 items-center gap-2 border-b border-hairline px-4 transition-colors duration-100 focus-within:bg-surface-1">
          <Search className="size-3.5 shrink-0 text-muted-foreground" />
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Filter shortcuts"
            aria-label="Filter shortcuts"
            autoComplete="off"
            spellCheck={false}
            className="h-full min-w-0 flex-1 border-0 bg-transparent p-0 text-body text-foreground outline-none placeholder:text-muted-foreground/60"
          />
        </label>

        <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto px-4 py-3">
          {groups.length === 0 ? (
            <p className="py-8 text-center text-body-sm text-muted-foreground">
              No shortcuts match “{query.trim()}”
            </p>
          ) : (
            // A lone group (say, after filtering) takes the full width, so its
            // keycaps sit at the dialog's edge, not beside an empty column.
            <div className={cn("gap-x-8", groups.length > 1 && "sm:columns-2")}>
              {groups.map((group) => (
                <section
                  key={group.category}
                  aria-label={group.label}
                  className="mb-4 break-inside-avoid"
                >
                  <Eyebrow className="mb-1">{group.label}</Eyebrow>
                  <ul>
                    {group.rows.map((row) => (
                      <li
                        key={row.id}
                        className="flex min-h-7 items-center justify-between gap-3 py-1 text-body-sm"
                      >
                        {/* Wraps instead of truncating: half of the 640px
                            sheet is too narrow for labels such as "New
                            workspace in current project". */}
                        <span className="min-w-0 leading-snug text-foreground/85">
                          {row.label}
                        </span>
                        <kbd className="shrink-0 rounded-sm border border-hairline-strong bg-surface-1 px-1.5 py-0.5 font-mono text-caption leading-none text-muted-foreground">
                          {row.keys}
                        </kbd>
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </div>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-2 border-t border-hairline bg-surface-1 px-4 py-2">
          <span className="flex-1 font-mono text-caption text-muted-foreground/60">
            {closeKeys && `${closeKeys.toLowerCase()} to close`}
          </span>
          <Button variant="ghost" size="sm" onClick={customize}>
            Customize…
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
