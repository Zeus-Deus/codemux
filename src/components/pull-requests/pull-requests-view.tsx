import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { Button } from "@/components/ui/button";
import { WindowChrome } from "@/components/layout/window-chrome";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { tzBodyLg, tzPageTitle } from "@/components/workspace/review/review-ui";
import { badgeKeys, rowKey, type PrRow } from "@/lib/pr-overview";
import { usePrOverview, type PrStateFilter } from "@/lib/pr-overview-query";
import { PrList } from "./pr-list";
import { PrTabStrip } from "./pr-tab-strip";
import { PrDetailColumn } from "./pr-detail-column";

/** The canvas measure. Wide enough for a title plus its state label,
 *  narrow enough to leave the detail column the room it needs. */
const DEFAULT_LIST_WIDTH = 452;
const MIN_LIST_WIDTH = 340;
const MAX_LIST_WIDTH = 720;

/** Anything Escape might reasonably mean "stop editing" rather than
 *  "leave the page" for. */
function isEditableElement(el: Element | null | undefined): boolean {
  if (!el || !(el instanceof HTMLElement)) return false;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return el.isContentEditable;
}

/** Inside a Radix overlay — a dialog, sheet, popover, dropdown or the
 *  wrapper any of them are portalled into. */
function isInsideOverlay(el: Element | null | undefined): boolean {
  if (!el || typeof el.closest !== "function") return false;
  return (
    el.closest(
      '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [data-radix-popper-content-wrapper]',
    ) != null
  );
}

/**
 * Pull requests, across every project you have open.
 *
 * A first-class destination like Automations and Workspaces: full
 * screen, its own sidebar entry, Escape to leave. The detail column is
 * the workspace Review panel's own component at a wider measure — the
 * same surface, not a second implementation of it.
 */
export function PullRequestsView() {
  const setShowPullRequests = useUIStore((s) => s.setShowPullRequests);
  const pendingSelection = useUIStore((s) => s.pendingPrSelection);
  const clearPendingPrSelection = useUIStore((s) => s.clearPendingPrSelection);
  const markPrBadgeSeen = useUIStore((s) => s.markPrBadgeSeen);

  const [stateFilter, setStateFilter] = useState<PrStateFilter>("open");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [tabKeys, setTabKeys] = useState<string[]>([]);
  const [listWidth, setListWidth] = useState(DEFAULT_LIST_WIDTH);

  const {
    rows,
    viewerByRoot,
    failures,
    roots,
    updatedAt,
    carried,
    carriedAt,
    allRootsFailed,
    refreshFailed,
    isLoading,
    refresh,
  } = usePrOverview(true, stateFilter);

  // Escape closes, matching the other full-screen destinations — but
  // only when nothing nearer to the user wanted the key first.
  //
  // This page is full of things that own Escape locally: the merge and
  // submit sheets, the thread reply box, the line composer, the image
  // lightbox. A page-level listener that fires regardless closes the
  // whole destination out from under them and takes the typed text with
  // it, which is exactly the promise the review surfaces are built on
  // ("anything typed survives everything"). So the page declines the key
  // in four cases, and this guard is why per-component workarounds are
  // no longer the thing standing between a reply draft and oblivion.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // 1. Something closer to the event already claimed it.
      if (event.defaultPrevented) return;

      const target = event.target as HTMLElement | null;
      const focused = document.activeElement as HTMLElement | null;

      // 2. The key belongs to whatever is being typed into.
      if (isEditableElement(target) || isEditableElement(focused)) return;

      // 3. Focus is inside an overlay that dismisses itself on Escape.
      //    Radix does not `preventDefault` when it closes a dialog, so
      //    `defaultPrevented` alone would not catch a sheet.
      if (isInsideOverlay(target) || isInsideOverlay(focused)) return;

      // 4. A modal is open even though focus escaped it. The DOM has not
      //    been updated with the close Radix just scheduled, so an open
      //    dialog here means the key was for that dialog.
      if (document.querySelector('[role="dialog"][data-state="open"]')) return;

      setShowPullRequests(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowPullRequests]);

  // Which workspace, if any, is standing on each branch. Resolved once
  // here rather than per row: the rows would otherwise each subscribe to
  // the whole workspace list.
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const workspaceByBranch = useMemo(() => {
    const map = new Map<string, string>();
    for (const ws of workspaces ?? []) {
      const root = ws.project_root ?? ws.cwd;
      if (root && ws.git_branch) map.set(`${root}\0${ws.git_branch}`, ws.workspace_id);
    }
    return map;
  }, [workspaces]);

  const byKey = useMemo(() => {
    const map = new Map<string, PrRow>();
    for (const row of rows) map.set(rowKey(row), row);
    return map;
  }, [rows]);

  // Opening the page is the acknowledgement the badge was waiting for.
  // Re-run as rows arrive so the count stays at zero while you are
  // looking at the very thing it was counting.
  useEffect(() => {
    markPrBadgeSeen(badgeKeys(rows, viewerByRoot));
  }, [rows, viewerByRoot, markPrBadgeSeen]);

  const openRow = useCallback((row: PrRow) => {
    const key = rowKey(row);
    setSelectedKey(key);
    setTabKeys((keys) => (keys.includes(key) ? keys : [...keys, key]));
  }, []);

  // The palette can ask for a specific pull request; honour it once the
  // row it names has actually loaded.
  useEffect(() => {
    if (!pendingSelection) return;
    const row = byKey.get(rowKey(pendingSelection));
    if (!row) return;
    openRow(row);
    clearPendingPrSelection();
  }, [pendingSelection, byKey, openRow, clearPendingPrSelection]);

  // First load lands on the first thing that wants something from you.
  useEffect(() => {
    if (selectedKey || pendingSelection || rows.length === 0) return;
    const first = rows.find((row) => {
      const viewer = viewerByRoot.get(row.projectRoot);
      return viewer && row.review_requested_from.some(
        (login) => login.toLowerCase() === viewer.toLowerCase(),
      );
    });
    if (first) openRow(first);
  }, [rows, viewerByRoot, selectedKey, pendingSelection, openRow]);

  // ── Rows that have left the list but are still open in a tab ──
  //
  // Merging or closing a pull request drops it out of the default "open"
  // filter on the very next refresh. Resolving the open tabs from the
  // filtered list alone therefore emptied the page the instant you did
  // the thing you came here to do: the row vanished, `selected` went
  // null, the tab strip went with it, and the built merged/closed detail
  // — the confirmation you were waiting for — was replaced by "Pick a
  // pull request".
  //
  // So every row a tab has pointed at is remembered, and a tab keeps
  // rendering from that copy once the list stops carrying it. The fresh
  // row always wins while it exists, and the cache is pruned to the open
  // tabs so it can't outgrow what's on screen.
  const lastKnown = useRef(new Map<string, PrRow>());
  useEffect(() => {
    const open = new Set(tabKeys);
    for (const key of open) {
      const row = byKey.get(key);
      if (row) lastKnown.current.set(key, row);
    }
    for (const key of [...lastKnown.current.keys()]) {
      if (!open.has(key)) lastKnown.current.delete(key);
    }
  }, [byKey, tabKeys]);

  const rowFor = useCallback(
    (key: string): PrRow | null => byKey.get(key) ?? lastKnown.current.get(key) ?? null,
    [byKey],
  );

  const selected = selectedKey ? rowFor(selectedKey) : null;
  const tabs = tabKeys.map(rowFor).filter((row): row is PrRow => row != null);

  const closeTab = (key: string) => {
    setTabKeys((keys) => {
      const next = keys.filter((k) => k !== key);
      if (key === selectedKey) {
        const index = keys.indexOf(key);
        // The neighbour, so closing a tab doesn't dump you back to
        // nothing while you are working through a queue.
        setSelectedKey(next[Math.min(index, next.length - 1)] ?? null);
      }
      return next;
    });
  };

  const detailRef = useRef<HTMLDivElement>(null);
  const startResize = (event: React.PointerEvent) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = listWidth;
    const onMove = (moveEvent: PointerEvent) => {
      const next = Math.min(
        Math.max(startWidth + moveEvent.clientX - startX, MIN_LIST_WIDTH),
        MAX_LIST_WIDTH,
      );
      setListWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <WindowChrome />
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 pb-2 pt-7">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close pull requests"
          className="text-muted-foreground hover:bg-muted/50 hover:text-foreground"
          onClick={() => setShowPullRequests(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <span className={cn("font-semibold tracking-tight text-foreground", tzPageTitle)}>
          Pull requests
        </span>
      </div>

      <div className="flex min-h-0 flex-1">
        <div
          className="flex min-h-0 shrink-0 flex-col border-r border-border/40"
          style={{ width: listWidth }}
        >
          <PrList
            rows={rows}
            viewerByRoot={viewerByRoot}
            workspaceByBranch={workspaceByBranch}
            failures={failures}
            hostCount={roots.length}
            updatedAt={updatedAt}
            carried={carried}
            carriedAt={carriedAt}
            allRootsFailed={allRootsFailed}
            refreshFailed={refreshFailed}
            isLoading={isLoading}
            selectedKey={selectedKey}
            stateFilter={stateFilter}
            onStateFilter={setStateFilter}
            onSelect={openRow}
            onOpenDetail={() => detailRef.current?.focus()}
            onRefresh={refresh}
          />
        </div>

        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize the list"
          data-testid="pr-list-resizer"
          onPointerDown={startResize}
          className="w-1 shrink-0 cursor-col-resize transition-colors hover:bg-foreground/20"
        />

        <div
          ref={detailRef}
          tabIndex={-1}
          className="flex min-h-0 min-w-0 flex-1 flex-col outline-none"
        >
          {selected ? (
            <>
              <PrTabStrip
                tabs={tabs}
                activeKey={selectedKey}
                candidates={rows}
                onSelect={openRow}
                onClose={closeTab}
                onOpenInBrowser={() => {
                  openUrl(selected.url).catch((err) => toast.error(String(err)));
                }}
              />
              {/* No scroll here: the detail column scrolls its own tab
                  body and keeps its action bar on the bottom edge. */}
              <div className="flex min-h-0 flex-1 flex-col">
                <PrDetailColumn
                  key={selectedKey}
                  row={selected}
                  existingWorkspaceId={
                    (selected.head_branch &&
                      workspaceByBranch.get(
                        `${selected.projectRoot}\0${selected.head_branch}`,
                      )) ||
                    null
                  }
                  viewerLogin={viewerByRoot.get(selected.projectRoot) ?? null}
                />
              </div>
            </>
          ) : (
            <div className="flex flex-1 items-center justify-center px-6">
              <p className={cn("max-w-sm text-center leading-relaxed text-muted-foreground", tzBodyLg)}>
                Pick a pull request on the left to read it here. ↑↓ moves through the
                list, ↵ opens the one you're on.
              </p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
