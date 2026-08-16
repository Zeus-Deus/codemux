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

  // Escape closes, matching the other full-screen destinations.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setShowPullRequests(false);
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

  const selected = selectedKey ? byKey.get(selectedKey) ?? null : null;
  const tabs = tabKeys
    .map((key) => byKey.get(key))
    .filter((row): row is PrRow => row != null);

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
