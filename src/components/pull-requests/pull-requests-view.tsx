import { useMobileLayout } from "@/hooks/use-mobile-layout";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useQueryClient } from "@tanstack/react-query";

import { UtilityPage } from "@/components/layout/utility-page";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { toast } from "@/lib/toast";
import { badgeKeys, rowKey, type PrRow } from "@/lib/pr-overview";
import {
  prHistoryKey,
  prOverviewKey,
  usePrOverview,
  type PrStateFilter,
} from "@/lib/pr-overview-query";
import { listPrsOverview, listPullRequests } from "@/tauri/commands";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { PrList } from "./pr-list";
import { PrTabStrip } from "./pr-tab-strip";
import { PrDetailColumn } from "./pr-detail-column";

/** The detail panel opens at this share of the page, and the list keeps
 *  at least `MIN_LIST_WIDTH` beside it however wide the panel is dragged. */
const DEFAULT_PANEL_SHARE = 0.55;
const MIN_PANEL_WIDTH = 440;
const MIN_LIST_WIDTH = 360;

/**
 * Pull requests, across every project you have open.
 *
 * A utility page like Automations: it opens beside the sidebar, and the
 * sidebar's Back (or Escape) leaves it. The list is the page; opening a
 * pull request slides its detail in on the right — the workspace Review
 * panel's own component at a wider measure, the same surface, not a
 * second implementation of it.
 */
export function PullRequestsView() {
  const mobile = useMobileLayout();
  const setShowPullRequests = useUIStore((s) => s.setShowPullRequests);
  const pendingSelection = useUIStore((s) => s.pendingPrSelection);
  const clearPendingPrSelection = useUIStore((s) => s.clearPendingPrSelection);
  const markPrBadgeSeen = useUIStore((s) => s.markPrBadgeSeen);

  const [stateFilter, setStateFilter] = useState<PrStateFilter>("open");
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [tabKeys, setTabKeys] = useState<string[]>([]);
  // Whether the detail panel is showing. On mobile it replaces the list.
  const [panelOpen, setPanelOpen] = useState(false);
  // null until the user drags it: the panel then follows the page's width.
  const [panelWidth, setPanelWidth] = useState<number | null>(null);

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
    rateLimitedUntil,
    isLoading,
    refresh,
    // "page": this is the surface someone is actually looking at, and
    // the only one that earns the fast cadence. Everything else sharing
    // these rows — the badge, the toasts, the palette — runs in the
    // slower `watch` mode by default.
  } = usePrOverview(true, stateFilter, "page");

  // Escape closes, matching the other full-screen destinations — but
  // only when nothing nearer to the user wanted the key first.
  //
  // This page is full of things that own Escape locally: the merge and
  // submit sheets, the thread reply box, the line composer, the image
  // lightbox. A page-level listener that fires regardless closes the
  // whole destination out from under them and takes the typed text with
  // it, which is exactly the promise the review surfaces are built on
  // ("anything typed survives everything"). So the page declines the key
  // whenever `escapeClaimedElsewhere` says an editor, overlay or open
  // dialog owns it — that guard, not per-component workarounds, is what
  // stands between a reply draft and oblivion.
  //
  // With a pull request open, the first Escape closes its panel and the
  // second leaves the page.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || escapeClaimedElsewhere(event)) return;
      if (panelOpen) setPanelOpen(false);
      else setShowPullRequests(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [panelOpen, setShowPullRequests]);

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
    setPanelOpen(true);
    setTabKeys((keys) => (keys.includes(key) ? keys : [...keys, key]));
  }, []);

  // ↑↓ in the list: with the panel open it follows the cursor, as it
  // always has; closed, the cursor just moves and ↵ opens.
  const moveTo = useCallback(
    (row: PrRow) => {
      if (panelOpen) openRow(row);
      else setSelectedKey(rowKey(row));
    },
    [panelOpen, openRow],
  );

  // The palette can ask for a specific pull request; honour it once the
  // row it names has actually loaded.
  useEffect(() => {
    if (!pendingSelection) return;
    const row = byKey.get(rowKey(pendingSelection));
    if (!row) return;
    openRow(row);
    clearPendingPrSelection();
  }, [pendingSelection, byKey, openRow, clearPendingPrSelection]);

  // A link can name a pull request the list doesn't hold: one opened
  // since the last poll, or one already merged out of the open filter.
  // Ask that repository directly — open list first, then the history,
  // widening the filter if that is where it lives — and only hand the
  // link to the browser once neither has it. Whatever the fetch adds to
  // the cache reaches `byKey`, and the effect above does the opening.
  const queryClient = useQueryClient();
  const lookedUp = useRef<string | null>(null);
  useEffect(() => {
    if (!pendingSelection) {
      lookedUp.current = null;
      return;
    }
    const key = rowKey(pendingSelection);
    if (byKey.has(key) || lookedUp.current === key) return;
    lookedUp.current = key;

    const selection = pendingSelection;
    const { projectRoot, number, url } = selection;
    const stillWanted = () => useUIStore.getState().pendingPrSelection === selection;
    void (async () => {
      const open = await queryClient
        .fetchQuery({
          queryKey: prOverviewKey(projectRoot),
          queryFn: () => listPrsOverview(projectRoot),
          staleTime: 0,
        })
        .catch(() => null);
      if (!stillWanted() || open?.items.some((item) => item.number === number)) return;

      const history = await queryClient
        .fetchQuery({
          queryKey: prHistoryKey(projectRoot, "all"),
          queryFn: () => listPullRequests(projectRoot, "all"),
          staleTime: 0,
        })
        .catch(() => null);
      if (!stillWanted()) return;
      if (history?.some((pr) => pr.number === number)) {
        setStateFilter("all");
        return;
      }

      clearPendingPrSelection();
      if (!url) {
        toast.error(`Couldn't find #${number}`);
        return;
      }
      toast.info(`Opening #${number} in the browser`, {
        description: "Codemux couldn't load it from the host.",
      });
      openUrl(url).catch((err) => toast.error(String(err)));
    })();
  }, [pendingSelection, byKey, queryClient, clearPendingPrSelection]);

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
      if (next.length === 0) setPanelOpen(false);
      return next;
    });
  };

  const detailRef = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const startResize = (event: React.PointerEvent) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = detailRef.current?.offsetWidth ?? MIN_PANEL_WIDTH;
    const bodyWidth = bodyRef.current?.clientWidth ?? window.innerWidth;
    const onMove = (moveEvent: PointerEvent) => {
      const next = Math.min(
        Math.max(startWidth - (moveEvent.clientX - startX), MIN_PANEL_WIDTH),
        Math.max(MIN_PANEL_WIDTH, bodyWidth - MIN_LIST_WIDTH),
      );
      setPanelWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  };

  // ↵ in the list: open the row under the cursor and hand it the keyboard.
  const openDetail = () => {
    if (selected) openRow(selected);
    requestAnimationFrame(() => detailRef.current?.focus());
  };

  const showPanel = panelOpen && selected !== null;

  return (
    <UtilityPage
      title="Pull requests"
      backLabel={mobile && showPanel ? "Back to pull requests" : "Close pull requests"}
      onBack={() => (mobile && showPanel ? setPanelOpen(false) : setShowPullRequests(false))}
    >
      <div ref={bodyRef} className="flex min-h-0 flex-1">
        <div
          className="flex min-h-0 min-w-0 flex-1 flex-col"
          style={{ display: mobile && showPanel ? "none" : undefined }}
        >
          <PrList
            rows={rows}
            viewerByRoot={viewerByRoot}
            workspaceByBranch={workspaceByBranch}
            failures={failures}
            rateLimitedUntil={rateLimitedUntil}
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
            onSelect={moveTo}
            onOpen={openRow}
            onOpenDetail={openDetail}
            onRefresh={refresh}
          />
        </div>

        {showPanel && (
          <>
            <div
              hidden={mobile}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize the pull request panel"
              data-testid="pr-list-resizer"
              onPointerDown={startResize}
              className="w-1 shrink-0 cursor-col-resize border-l border-border/60 transition-colors duration-150 hover:bg-foreground/20"
            />
            <div
              ref={detailRef}
              tabIndex={-1}
              data-testid="pr-detail-panel"
              className="flex min-h-0 min-w-0 flex-col bg-background outline-none"
              style={
                mobile
                  ? { flex: 1 }
                  : {
                      width: panelWidth ?? `${DEFAULT_PANEL_SHARE * 100}%`,
                      minWidth: MIN_PANEL_WIDTH,
                      maxWidth: `calc(100% - ${MIN_LIST_WIDTH}px)`,
                      flexShrink: 0,
                    }
              }
            >
              <PrTabStrip
                tabs={tabs}
                activeKey={selectedKey}
                candidates={rows}
                onSelect={openRow}
                onClose={closeTab}
                onClosePanel={() => setPanelOpen(false)}
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
            </div>
          </>
        )}
      </div>
    </UtilityPage>
  );
}
