import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  ArrowDownToLine,
  Cloud,
  Filter,
  Folder,
  LayoutGrid,
  Loader2,
  Plus,
  Search,
  Server,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import { useHostsStore } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";

import type { WorkspaceSyncView } from "@/tauri/commands";

import { WorkspaceOverviewRow } from "./workspace-overview-row";
import {
  detectDivergence,
  useOverviewItems,
  type DeviceBucket,
  type DivergenceInfo,
  type OverviewItem,
} from "./use-overview-items";
import { PullToDeviceDialog } from "./pull-to-device-dialog";
import { WelcomeBanner } from "./welcome-banner";
import { HowItWorksPopover } from "./how-it-works-popover";

/**
 * The Workspaces overview body — rendered full-screen by
 * `WorkspacesOverviewView`, reached from the left sidebar.
 *
 * Lists every workspace this account owns, across every device:
 * local rows from `app_state.workspaces` plus synced rows from the
 * `workspaces_sync` table (which the Rust background loop keeps in
 * sync with `/api/workspaces`). Workspaces are bucketed by host —
 * pushing a workspace to a host visibly migrates its card from
 * "This device" to the matching host section.
 */

type SortBy = "recent" | "name" | "branch";
type CreatedWithin = "all" | "1d" | "7d" | "30d" | "90d";
type HostFilter = "all" | "local" | string; // `string` = host server id
type StatusFilter = "all" | "attached" | "remote-host" | "dirty" | "sibling-device";

const CREATED_WITHIN_DAYS: Record<Exclude<CreatedWithin, "all">, number> = {
  "1d": 1,
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

export function WorkspacesOverviewSection() {
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? null);
  const activeWorkspaceId = useAppStore(
    (s) => s.appState?.active_workspace_id ?? null,
  );
  const initHosts = useHostsStore((s) => s.init);
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  // Filters
  const [search, setSearch] = useState("");
  const [projectFilter, setProjectFilter] = useState<string>("all");
  const [hostFilter, setHostFilter] = useState<HostFilter>("all");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [createdWithin, setCreatedWithin] = useState<CreatedWithin>("all");
  const [sortBy, setSortBy] = useState<SortBy>("recent");

  // Cross-device adoption dialog state — hoisted here so any
  // sibling-device row's `⋯ → Pull to this device` opens the same
  // shared modal. Holds the sync row whose adoption is in progress;
  // null = closed.
  const [pullRow, setPullRow] = useState<WorkspaceSyncView | null>(null);
  const handleRequestPull = useCallback(
    (item: Extract<OverviewItem, { kind: "remote" }>) => {
      setPullRow(item.sync);
    },
    [],
  );

  // Scroll-to-first-remote machinery — the empty local bucket's
  // "Pull from another device" CTA scrolls the overview to the
  // first remote row and asks the row to pulse for 2 seconds. The
  // ref-by-key map is populated by the row component on mount.
  const remoteRowRefs = useRef<Map<string, HTMLElement>>(new Map());
  // Set of remote row keys that should currently render the
  // pulse-attention animation. Cleared per-key on a timer so the
  // class re-renders accept new entries (e.g. another new sibling
  // syncing in while the previous one is still pulsing).
  const [pulseKeys, setPulseKeys] = useState<Set<string>>(
    () => new Set(),
  );
  const triggerPulse = useCallback((key: string) => {
    setPulseKeys((prev) => {
      const next = new Set(prev);
      next.add(key);
      return next;
    });
    window.setTimeout(() => {
      setPulseKeys((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }, 1800);
  }, []);
  const registerRemoteRow = useCallback(
    (key: string, el: HTMLElement | null) => {
      if (el) remoteRowRefs.current.set(key, el);
      else remoteRowRefs.current.delete(key);
    },
    [],
  );
  const handleScrollToFirstRemote = useCallback(() => {
    // Find the first remote row in render order (Map preserves
    // insertion order, which matches DeviceSection's render order
    // — locals first, then hosts in user order).
    const firstKey = Array.from(remoteRowRefs.current.keys()).find((k) =>
      k.startsWith("remote:"),
    );
    if (!firstKey) return;
    const el = remoteRowRefs.current.get(firstKey);
    el?.scrollIntoView({ behavior: "smooth", block: "center" });
    triggerPulse(firstKey);
  }, [triggerPulse]);

  // "Newly-appeared sibling-device row" detection lives further
  // down, after `allItems` is in scope (declared by useOverviewItems).
  const prevRemoteKeysRef = useRef<Set<string> | null>(null);

  // Eagerly load hosts so device-bucket labels resolve immediately.
  useEffect(() => {
    void initHosts();
  }, [initHosts]);

  // Unified item list — locals + sibling-device synced rows.
  const { items: allItems, hosts } = useOverviewItems();

  // Phase-4 divergence detection: rows that share a project_remote
  // and git_branch but have different HEADs get a warning chip.
  const divergenceByKey = useMemo<Map<string, DivergenceInfo>>(() => {
    const hostLabelFor = (hostServerId: string | null): string => {
      if (!hostServerId) return "this device";
      const h = hosts.find((host) => host.server_id === hostServerId);
      return h?.name ?? "another device";
    };
    return detectDivergence(allItems, hostLabelFor);
  }, [allItems, hosts]);

  // Pulse newly-appeared remote rows: when a sync tick brings in a
  // remote row this device hadn't seen before, briefly pulse it so
  // the user notices new content. Initial mount primes the ref
  // WITHOUT pulsing, otherwise every row would flash on first open
  // of the overview, which would defeat the affordance.
  useEffect(() => {
    const currentKeys = new Set(
      allItems
        .filter((it): it is Extract<OverviewItem, { kind: "remote" }> =>
          it.kind === "remote",
        )
        .map((it) => it.key),
    );
    const prev = prevRemoteKeysRef.current;
    if (prev !== null) {
      for (const key of currentKeys) {
        if (!prev.has(key)) triggerPulse(key);
      }
    }
    prevRemoteKeysRef.current = currentKeys;
  }, [allItems, triggerPulse]);

  // Project picker options — every distinct project path the
  // overview can see, regardless of whether it's local or remote.
  const projects = useMemo(() => {
    const seen = new Map<string, string>();
    for (const it of allItems) {
      if (it.projectPath && it.projectName) {
        seen.set(it.projectPath, it.projectName);
      }
    }
    return Array.from(seen.entries())
      .map(([path, name]) => ({ path, name }))
      .sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
      );
  }, [allItems]);

  // Filter pipeline.
  const filtered = useMemo<OverviewItem[]>(() => {
    const query = search.trim().toLowerCase();
    const cutoff =
      createdWithin === "all"
        ? null
        : Date.now() - CREATED_WITHIN_DAYS[createdWithin] * 86_400_000;

    return allItems.filter((it) => {
      // Project filter
      if (projectFilter !== "all") {
        if (!it.projectPath || it.projectPath !== projectFilter) return false;
      }

      // Host filter
      if (hostFilter === "local") {
        if (it.hostServerId !== null) return false;
      } else if (hostFilter !== "all") {
        if (it.hostServerId !== hostFilter) return false;
      }

      // Status filter
      if (statusFilter === "attached") {
        if (it.kind !== "local" || it.workspace.workspace_id !== activeWorkspaceId) {
          return false;
        }
      } else if (statusFilter === "remote-host") {
        if (it.hostServerId === null) return false;
      } else if (statusFilter === "sibling-device") {
        if (it.kind !== "remote") return false;
      } else if (statusFilter === "dirty") {
        if (it.kind === "local") {
          if (
            it.workspace.git_changed_files === 0 &&
            it.workspace.git_ahead === 0
          )
            return false;
        } else {
          // Remote-only rows have no live git stats; don't match "dirty".
          return false;
        }
      }

      // Time-window: created_at is on the sync row; for local rows
      // without a sync row yet (transient), use no filter rather
      // than incorrectly dropping them.
      if (cutoff !== null) {
        const created = it.kind === "local" ? it.sync?.created_at : it.sync.created_at;
        if (created) {
          const ts = parseTimestamp(created);
          if (ts !== null && ts < cutoff) return false;
        }
      }

      if (!query) return true;
      const title =
        it.kind === "local" ? it.workspace.title : it.sync.title;
      const branch =
        it.kind === "local"
          ? it.workspace.git_branch ?? ""
          : it.sync.git_branch ?? "";
      return (
        title.toLowerCase().includes(query) ||
        branch.toLowerCase().includes(query) ||
        (it.projectName?.toLowerCase().includes(query) ?? false)
      );
    });
  }, [
    allItems,
    search,
    projectFilter,
    hostFilter,
    statusFilter,
    createdWithin,
    activeWorkspaceId,
  ]);

  // Bucket the filtered list by host. Local first, then each
  // configured host in user order, then orphan/sibling buckets for
  // workspaces whose host isn't on this device yet.
  const buckets = useMemo<DeviceBucket[]>(() => {
    // Build bucket key → bucket. Pre-create the local + configured-
    // host buckets so empty configured hosts still show up.
    const byKey = new Map<string, DeviceBucket>();
    byKey.set("local", {
      key: "local",
      localHostId: null,
      hostServerId: null,
      label: "This device",
      sublabel: null,
      items: [],
      totalCount: 0,
      sortRank: 0,
    });
    hosts.forEach((h, idx) => {
      if (!h.server_id) return; // host hasn't synced yet — no cross-device id
      byKey.set(h.server_id, {
        key: h.server_id,
        localHostId: h.id,
        hostServerId: h.server_id,
        label: h.name,
        sublabel: h.ssh_target,
        items: [],
        totalCount: 0,
        sortRank: 1 + idx,
      });
    });

    // Count totals on the unfiltered list (for "X hidden by filter").
    for (const it of allItems) {
      const key = it.hostServerId ?? "local";
      let bucket = byKey.get(key);
      if (!bucket) {
        // Orphan host: workspace references a host_server_id we
        // don't know locally yet (the hosts-sync hasn't caught up,
        // or the host row was deleted). Create the bucket.
        bucket = {
          key,
          localHostId: null,
          hostServerId: it.hostServerId,
          label: it.hostServerId ? "Host not on this device" : "This device",
          sublabel: it.hostServerId
            ? `host #${it.hostServerId}`
            : null,
          items: [],
          totalCount: 0,
          sortRank: 999,
        };
        byKey.set(key, bucket);
      }
      bucket.totalCount += 1;
    }

    // Place filtered items.
    for (const it of filtered) {
      const key = it.hostServerId ?? "local";
      const bucket = byKey.get(key);
      if (bucket) bucket.items.push(it);
    }

    // Sort items inside each bucket.
    const sorter = makeSorter(sortBy, activeWorkspaceId);
    for (const bucket of byKey.values()) {
      bucket.items.sort(sorter);
    }

    // Keep configured-host buckets (localHostId != null) even when
    // empty — matches the intent stated above at the pre-create step:
    // a device the user has set up should be visible in the overview
    // the moment it's configured, not only after the first workspace
    // lands on it. Without this, an empty pandora bucket gets created
    // and then immediately stripped here, which is why the user only
    // ever saw "This device" until they pushed a workspace.
    //
    // Orphan buckets (localHostId == null, hostServerId != null) and
    // the special "local" bucket still follow the items/totalCount
    // rule so the overview doesn't render a dangling "This device"
    // row when the user has zero workspaces and zero remote rows.
    return Array.from(byKey.values())
      .filter(
        (b) =>
          b.localHostId !== null || b.items.length > 0 || b.totalCount > 0,
      )
      .sort((a, b) => a.sortRank - b.sortRank);
  }, [allItems, filtered, hosts, sortBy, activeWorkspaceId]);

  const totalShown = filtered.length;
  const totalAll = allItems.length;
  const hasActiveFilter =
    search.trim() !== "" ||
    projectFilter !== "all" ||
    hostFilter !== "all" ||
    statusFilter !== "all" ||
    createdWithin !== "all";

  const clearFilters = useCallback(() => {
    setSearch("");
    setProjectFilter("all");
    setHostFilter("all");
    setStatusFilter("all");
    setCreatedWithin("all");
  }, []);

  // ── Loading state ──────────────────────────────────────────────
  if (workspaces === null) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" />
        Loading workspaces…
      </div>
    );
  }

  // ── First-run empty state ──────────────────────────────────────
  if (totalAll === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6">
        <div className="max-w-sm space-y-4 text-center">
          <div className="mx-auto flex size-14 items-center justify-center rounded-2xl border border-border/50 bg-muted/40">
            <LayoutGrid className="size-6 text-muted-foreground/70" />
          </div>
          <div className="space-y-1.5">
            <h3 className="text-[15px] font-semibold tracking-tight text-foreground">
              No workspaces yet
            </h3>
            <p className="text-[12.5px] leading-relaxed text-muted-foreground/80">
              Workspaces hold a worktree, an agent, and any panes you open.
              Create one here, then push it to a host whenever you want it
              to run somewhere else — every device of your account shows up
              alongside this one.
            </p>
          </div>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            className="h-8 gap-1.5 text-[12.5px]"
            onClick={() => {
              setShowWorkspacesOverview(false);
              setShowNewWorkspaceDialog(true);
            }}
          >
            <Plus className="size-3.5" />
            New workspace
          </Button>
        </div>
      </div>
    );
  }

  // ── Main view ───────────────────────────────────────────────────
  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Filter bar */}
      <div className="shrink-0 border-b border-border/60 bg-background/60 px-6 py-3 backdrop-blur">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-2">
          {/* Search */}
          <div className="relative min-w-[220px] flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground/60" />
            <Input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search by name, branch, or project…"
              className="h-8 pl-8 text-[13px]"
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch("")}
                aria-label="Clear search"
                className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground/60 hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            )}
          </div>

          {/* Project filter */}
          <Select value={projectFilter} onValueChange={setProjectFilter}>
            <SelectTrigger className="h-8 w-[160px] text-[12.5px]">
              <SelectValue placeholder="All projects" />
            </SelectTrigger>
            <SelectContent position="popper" className="max-h-72">
              <SelectItem value="all">All projects</SelectItem>
              {projects.map((p) => (
                <SelectItem key={p.path} value={p.path}>
                  {p.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {/* Device filter */}
          <Select
            value={hostFilter}
            onValueChange={(v) => setHostFilter(v as HostFilter)}
          >
            <SelectTrigger className="h-8 w-[160px] text-[12.5px]">
              <SelectValue placeholder="All devices" />
            </SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value="all">All devices</SelectItem>
              <SelectItem value="local">This device</SelectItem>
              {hosts
                .filter((h) => h.server_id)
                .map((h) => (
                  <SelectItem key={h.server_id!} value={h.server_id!}>
                    {h.name}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>

          {/* Status filter */}
          <Select
            value={statusFilter}
            onValueChange={(v) => setStatusFilter(v as StatusFilter)}
          >
            <SelectTrigger className="h-8 w-[160px] text-[12.5px]">
              <SelectValue placeholder="Any status" />
            </SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value="all">Any status</SelectItem>
              <SelectItem value="attached">Currently open</SelectItem>
              <SelectItem value="remote-host">On a remote host</SelectItem>
              <SelectItem value="sibling-device">On another device</SelectItem>
              <SelectItem value="dirty">Has uncommitted work</SelectItem>
            </SelectContent>
          </Select>

          {/* Sort */}
          <Select
            value={sortBy}
            onValueChange={(v) => setSortBy(v as SortBy)}
          >
            <SelectTrigger className="h-8 w-[150px] text-[12.5px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent position="popper">
              <SelectItem value="recent">Recently active</SelectItem>
              <SelectItem value="name">Name (A–Z)</SelectItem>
              <SelectItem value="branch">Branch (A–Z)</SelectItem>
            </SelectContent>
          </Select>

          {hasActiveFilter && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-8 gap-1.5 text-[12px] text-muted-foreground"
              onClick={clearFilters}
            >
              <Filter className="size-3.5" />
              Clear
            </Button>
          )}
        </div>
      </div>

      {/* Result count + how-it-works + new-workspace shortcut */}
      <div className="shrink-0 border-b border-border/40 px-6 py-2">
        <div className="mx-auto flex max-w-6xl items-center justify-between gap-3">
          <div className="flex items-center gap-1.5">
            <p className="text-[11.5px] text-muted-foreground/70 tabular-nums">
              {totalShown === totalAll
                ? `${totalAll} ${totalAll === 1 ? "workspace" : "workspaces"}`
                : `${totalShown} of ${totalAll} shown`}
              {hosts.length > 0 && (
                <span className="ml-2 text-muted-foreground/50">
                  · {hosts.length}{" "}
                  {hosts.length === 1 ? "device" : "devices"} configured
                </span>
              )}
            </p>
            <HowItWorksPopover />
          </div>
          <div className="flex items-center gap-1.5">
            {/* Add Device is intentionally surfaced here as well as
                in Settings — a brand-new user who's just signed in
                shouldn't have to hunt through Settings to find the
                primary "make this useful" action. */}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 gap-1.5 text-[12px] text-muted-foreground hover:text-foreground"
              onClick={() => {
                setShowWorkspacesOverview(false);
                setShowSettings(true, "hosts");
              }}
            >
              <Server className="size-3.5" />
              Add device
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 gap-1.5 text-[12px] text-muted-foreground hover:text-foreground"
              onClick={() => {
                setShowWorkspacesOverview(false);
                setShowNewWorkspaceDialog(true);
              }}
            >
              <Plus className="size-3.5" />
              New workspace
            </Button>
          </div>
        </div>
      </div>

      {/* Body */}
      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-6">
        <WelcomeBanner
          deviceCount={hosts.length}
          siblingWorkspaceCount={
            allItems.filter((it) => it.kind === "remote").length
          }
          localWorkspaceCount={
            allItems.filter((it) => it.kind === "local").length
          }
        />
        <div className="mx-auto max-w-6xl space-y-8">
          {buckets.length === 0 ? (
            <EmptyFilters onClear={clearFilters} />
          ) : (
            buckets.map((bucket) => (
              <DeviceSection
                key={bucket.key}
                bucket={bucket}
                activeWorkspaceId={activeWorkspaceId}
                onCloseOverview={() => setShowWorkspacesOverview(false)}
                onRequestPull={handleRequestPull}
                onScrollToFirstRemote={handleScrollToFirstRemote}
                hasAnySibling={allItems.some((i) => i.kind === "remote")}
                registerRemoteRow={registerRemoteRow}
                pulseKeys={pulseKeys}
                divergenceByKey={divergenceByKey}
              />
            ))
          )}
        </div>
      </div>

      <PullToDeviceDialog
        syncRow={pullRow}
        onOpenChange={(open) => {
          if (!open) setPullRow(null);
        }}
      />
    </div>
  );
}

// ── Device-grouped section ──────────────────────────────────────

function DeviceSection({
  bucket,
  activeWorkspaceId,
  onCloseOverview,
  onRequestPull,
  onScrollToFirstRemote,
  hasAnySibling,
  registerRemoteRow,
  pulseKeys,
  divergenceByKey,
}: {
  bucket: DeviceBucket;
  activeWorkspaceId: string | null;
  onCloseOverview: () => void;
  onRequestPull: (
    item: Extract<OverviewItem, { kind: "remote" }>,
  ) => void;
  /** Trigger the "Pull from another device" CTA: scroll the overview
   *  to the first sibling-device row and briefly pulse it. */
  onScrollToFirstRemote: () => void;
  /** True when the overview contains at least one sibling-device
   *  row — drives whether the "Pull from another device" CTA shows
   *  in an empty local bucket. */
  hasAnySibling: boolean;
  /** Per-row ref registration so the parent can scrollIntoView a
   *  specific remote row when the user clicks "Pull from another
   *  device" in an empty bucket. */
  registerRemoteRow: (key: string, el: HTMLElement | null) => void;
  /** Set of remote-row keys that should currently pulse — either
   *  from the "Pull from another device" CTA scrolling to them, or
   *  from a sync tick bringing in a new sibling-device workspace. */
  pulseKeys: Set<string>;
  /** Phase-4 divergence info, keyed by row identity. The row
   *  component reads its own key (`local:<workspace_id>` or
   *  `remote:<sync.id>`) and renders a warning chip when present. */
  divergenceByKey: Map<string, DivergenceInfo>;
}) {
  const isLocal = bucket.hostServerId === null;
  const hiddenByFilter = bucket.totalCount - bucket.items.length;

  return (
    <section>
      <header className="mb-2.5 flex items-end justify-between gap-3 border-b border-border/40 pb-2">
        <div className="flex min-w-0 items-center gap-2.5">
          <span
            className={cn(
              "flex size-7 shrink-0 items-center justify-center rounded-md border",
              isLocal
                ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-400"
                : "border-sky-500/25 bg-sky-500/8 text-sky-300",
            )}
          >
            {isLocal ? (
              <LocalDeviceGlyph />
            ) : (
              <Cloud className="size-3.5" aria-hidden />
            )}
          </span>
          <div className="min-w-0">
            <h3 className="truncate text-[13px] font-semibold tracking-tight text-foreground">
              {bucket.label}
            </h3>
            {bucket.sublabel && (
              <p className="truncate font-mono text-[10.5px] text-muted-foreground/60">
                {bucket.sublabel}
              </p>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-baseline gap-2 text-[10px] uppercase tracking-[0.14em] text-muted-foreground/55">
          {hiddenByFilter > 0 && (
            <span
              title={`${hiddenByFilter} hidden by filter`}
              className="rounded bg-muted/40 px-1.5 py-0.5 text-warning/80 normal-case tracking-normal"
            >
              {hiddenByFilter} filtered
            </span>
          )}
          <span className="tabular-nums text-muted-foreground/70 normal-case tracking-normal">
            {bucket.items.length}{" "}
            {bucket.items.length === 1 ? "workspace" : "workspaces"}
          </span>
        </div>
      </header>

      {bucket.items.length === 0 ? (
        <EmptyBucketCTA
          isLocal={isLocal}
          hiddenByFilter={hiddenByFilter}
          hasAnySibling={hasAnySibling}
          bucketLabel={bucket.label}
          onScrollToFirstRemote={onScrollToFirstRemote}
          onCloseOverview={onCloseOverview}
        />
      ) : (
        (() => {
          // Within a device, cluster workspaces that belong to the same
          // project so siblings (e.g. a repo's root checkout + its
          // worktrees) visibly read as one project rather than as
          // unrelated cards. Only projects with 2+ workspaces get a
          // header; lone workspaces fall through to a plain grid so a
          // device full of one-off projects isn't littered with
          // single-item headers.
          const { clustered, rest } = partitionByProject(bucket.items);
          const rowGridProps = {
            activeWorkspaceId,
            onCloseOverview,
            onRequestPull,
            registerRemoteRow,
            pulseKeys,
            divergenceByKey,
          };
          if (clustered.length === 0) {
            return <RowGrid items={bucket.items} {...rowGridProps} />;
          }
          return (
            <div className="space-y-4">
              {clustered.map((group) => (
                <div key={group.key} className="space-y-1.5">
                  <ProjectGroupHeader
                    name={group.name}
                    count={group.items.length}
                  />
                  <RowGrid items={group.items} {...rowGridProps} />
                </div>
              ))}
              {rest.length > 0 && (
                <RowGrid items={rest} {...rowGridProps} />
              )}
            </div>
          );
        })()
      )}
    </section>
  );
}

/**
 * Split a bucket's items into same-project clusters (2+ workspaces
 * sharing a project name) plus the leftover singletons. Items keep
 * their incoming sort order within each group; `rest` is filtered
 * from the original list so its order is preserved too. Project
 * identity is the stable `projectKey` (the deterministic `project_uid`
 * when known, else the project path/name), so a root checkout and its
 * worktrees — which carry different paths but the same project_uid —
 * cluster together, while two unrelated repos that merely share a
 * basename do not. Falls back to the project name when no key exists.
 */
function partitionByProject(items: OverviewItem[]): {
  clustered: { key: string; name: string; items: OverviewItem[] }[];
  rest: OverviewItem[];
} {
  const groups = new Map<string, { name: string; items: OverviewItem[] }>();
  for (const it of items) {
    const name = it.projectName;
    if (!name) continue;
    const key = (it.projectKey ?? name).toLowerCase();
    const g = groups.get(key);
    if (g) g.items.push(it);
    else groups.set(key, { name, items: [it] });
  }

  const clustered: { key: string; name: string; items: OverviewItem[] }[] = [];
  const clusteredKeys = new Set<string>();
  for (const [key, g] of groups) {
    if (g.items.length >= 2) {
      clustered.push({ key, name: g.name, items: g.items });
      clusteredKeys.add(key);
    }
  }
  clustered.sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );

  const rest = items.filter((it) => {
    const name = it.projectName;
    if (!name) return true;
    return !clusteredKeys.has((it.projectKey ?? name).toLowerCase());
  });

  return { clustered, rest };
}

/** Subtle caption above a same-project cluster inside a device bucket. */
function ProjectGroupHeader({
  name,
  count,
}: {
  name: string;
  count: number;
}) {
  return (
    <div className="flex items-center gap-1.5 pl-0.5 text-[11px] text-muted-foreground/70">
      <Folder className="size-3 text-muted-foreground/50" aria-hidden />
      <span className="truncate font-medium text-foreground/75">{name}</span>
      <span className="tabular-nums text-muted-foreground/45">{count}</span>
    </div>
  );
}

/** The 2-up responsive grid of workspace cards shared by every
 *  project cluster and the leftover-singletons grid. */
function RowGrid({
  items,
  activeWorkspaceId,
  onCloseOverview,
  onRequestPull,
  registerRemoteRow,
  pulseKeys,
  divergenceByKey,
}: {
  items: OverviewItem[];
  activeWorkspaceId: string | null;
  onCloseOverview: () => void;
  onRequestPull: (item: Extract<OverviewItem, { kind: "remote" }>) => void;
  registerRemoteRow: (key: string, el: HTMLElement | null) => void;
  pulseKeys: Set<string>;
  divergenceByKey: Map<string, DivergenceInfo>;
}) {
  return (
    <ul className="grid grid-cols-1 gap-1.5 md:grid-cols-2">
      {items.map((it) => (
        <li
          key={it.key}
          ref={
            it.kind === "remote"
              ? (el) => registerRemoteRow(it.key, el)
              : undefined
          }
          className={cn(
            it.kind === "remote" && pulseKeys.has(it.key)
              ? "cm-pulse-attention"
              : null,
          )}
        >
          <WorkspaceOverviewRow
            item={it}
            isAttached={
              it.kind === "local" &&
              it.workspace.workspace_id === activeWorkspaceId
            }
            onAfterOpen={onCloseOverview}
            onRequestPull={onRequestPull}
            divergence={
              divergenceByKey.get(
                it.kind === "local"
                  ? `local:${it.workspace.workspace_id}`
                  : `remote:${it.sync.id}`,
              ) ?? null
            }
          />
        </li>
      ))}
    </ul>
  );
}

/**
 * Empty-bucket CTA — the row of action chips that renders inside a
 * device section whose item list is currently empty. Lives here
 * (rather than in DeviceSection inline) so the local vs remote vs
 * "everything filtered out" variants have a single readable place.
 */
function EmptyBucketCTA({
  isLocal,
  hiddenByFilter,
  hasAnySibling,
  bucketLabel,
  onScrollToFirstRemote,
  onCloseOverview,
}: {
  isLocal: boolean;
  hiddenByFilter: number;
  hasAnySibling: boolean;
  bucketLabel: string;
  onScrollToFirstRemote: () => void;
  onCloseOverview: () => void;
}) {
  const setShowNewWorkspaceDialog = useUIStore(
    (s) => s.setShowNewWorkspaceDialog,
  );
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );

  // Filtered-out state — single sentence, no CTAs (the global
  // "Clear filters" affordance already lives in the filter bar).
  if (hiddenByFilter > 0) {
    return (
      <p className="rounded-md border border-dashed border-border/40 px-3 py-4 text-center text-[12px] text-muted-foreground/60">
        Every workspace in {bucketLabel} is hidden by the current
        filter.
      </p>
    );
  }

  // Local-bucket empty state — offer New workspace + Pull from
  // another device (when siblings exist) as the two natural next
  // actions a brand-new device has.
  if (isLocal) {
    return (
      <div className="rounded-lg border border-dashed border-border/50 bg-muted/20 px-4 py-5 text-center space-y-3">
        <p className="text-[12.5px] text-muted-foreground/85">
          No workspaces on this device yet.
        </p>
        <div className="flex flex-wrap items-center justify-center gap-2">
          <Button
            type="button"
            variant="secondary"
            size="sm"
            className="h-7 gap-1.5 px-3 text-[12px]"
            onClick={() => {
              setShowWorkspacesOverview(false);
              onCloseOverview();
              setShowNewWorkspaceDialog(true);
            }}
          >
            <Plus className="size-3" />
            New workspace
          </Button>
          {hasAnySibling && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 gap-1.5 px-3 text-[12px]"
              onClick={onScrollToFirstRemote}
            >
              <ArrowDownToLine className="size-3" />
              Pull from another device
            </Button>
          )}
        </div>
      </div>
    );
  }

  // Remote-bucket empty state — device has been added but nothing
  // has been pushed there yet. The clearest next step is opening
  // an existing workspace and pushing it from the context menu;
  // we frame the empty state with a hint rather than try to
  // construct a "push" CTA (which needs a workspace pre-selected).
  return (
    <div className="rounded-lg border border-dashed border-border/50 bg-muted/20 px-4 py-5 text-center space-y-2">
      <p className="text-[12.5px] text-muted-foreground/85">
        No workspaces pushed to {bucketLabel} yet.
      </p>
      <p className="text-[11px] text-muted-foreground/65 leading-relaxed">
        Right-click any local workspace →{" "}
        <span className="font-medium">Move to {bucketLabel}</span> to
        send it here. You can pull it back from any device anytime.
      </p>
    </div>
  );
}

function EmptyFilters({ onClear }: { onClear: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed border-border/40 bg-muted/10 px-6 py-12 text-center">
      <Folder className="size-6 text-muted-foreground/40" />
      <div className="space-y-1">
        <p className="text-[13px] font-medium text-foreground">
          No workspaces match these filters
        </p>
        <p className="text-[11.5px] text-muted-foreground/70">
          Try a different search, or clear the filter to see everything.
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="h-7 gap-1.5 text-[12px]"
        onClick={onClear}
      >
        <X className="size-3.5" />
        Clear filters
      </Button>
    </div>
  );
}

// ── Sorting helpers ─────────────────────────────────────────────

function makeSorter(
  by: SortBy,
  activeWorkspaceId: string | null,
): (a: OverviewItem, b: OverviewItem) => number {
  return (a, b) => {
    const titleA = itemTitle(a).toLowerCase();
    const titleB = itemTitle(b).toLowerCase();

    if (by === "name") {
      return titleA.localeCompare(titleB);
    }
    if (by === "branch") {
      const ba = (itemBranch(a) ?? "").toLowerCase();
      const bb = (itemBranch(b) ?? "").toLowerCase();
      if (!ba && bb) return 1;
      if (ba && !bb) return -1;
      return ba.localeCompare(bb);
    }
    // "recent" — locals come ahead of remote-only because we know
    // more about them; among locals, the currently-attached row
    // floats to the top, then dirty/notifying rows, then by name.
    const rankA = recencyRank(a, activeWorkspaceId);
    const rankB = recencyRank(b, activeWorkspaceId);
    if (rankA !== rankB) return rankB - rankA;
    return titleA.localeCompare(titleB);
  };
}

function recencyRank(
  it: OverviewItem,
  activeWorkspaceId: string | null,
): number {
  if (it.kind === "remote") return 0;
  let rank = 1; // local baseline
  if (it.workspace.workspace_id === activeWorkspaceId) rank += 10;
  rank += it.workspace.notification_count;
  if (it.workspace.git_changed_files > 0) rank += 1;
  return rank;
}

function itemTitle(it: OverviewItem): string {
  return it.kind === "local" ? it.workspace.title : it.sync.title;
}

function itemBranch(it: OverviewItem): string | null {
  return it.kind === "local"
    ? it.workspace.git_branch
    : it.sync.git_branch;
}

function parseTimestamp(s: string): number | null {
  // The server emits ISO; SQLite emits "YYYY-MM-DD HH:MM:SS".
  // `Date.parse` handles ISO; for the SQLite shape add a `T` and `Z`.
  const iso = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(s)
    ? s.replace(" ", "T") + "Z"
    : s;
  const t = Date.parse(iso);
  return Number.isNaN(t) ? null : t;
}

// ── Inline glyphs ───────────────────────────────────────────────

function LocalDeviceGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="14"
      height="14"
      aria-hidden
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="2.5" y="3" width="11" height="7" rx="1.2" />
      <path d="M1 12.5h14" />
    </svg>
  );
}
