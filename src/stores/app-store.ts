import { useMemo } from "react";
import { create } from "zustand";
import type {
  AppStateDelta,
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
  SurfaceSnapshot,
} from "@/tauri/types";
import { basename, tailSegments, segmentCount } from "@/lib/path";
import { shareStructural } from "@/lib/structural-share";
import type { HostView } from "@/tauri/commands";

interface AppStore {
  appState: AppStateSnapshot | null;
  /** Cached `$HOME` string from the `get_home_dir` Tauri call. Null
   *  until hydrated at App mount. Consumers (project-group selector,
   *  lazy-draft home detection) should treat null as "not yet known"
   *  and fall back to today's path-basename grouping. */
  homeDir: string | null;
  /** Workspace id currently being pushed to or pulled from a remote
   *  host. Drives the spinner icon on the sidebar row so the user
   *  sees the operation is in flight. Null when no push/pull is
   *  running. Set by the workspace context menu's Move/Pull handlers
   *  and cleared in the completion callback (success or failure). */
  workspacePushPullInFlight: string | null;
  /** Timestamp (Date.now()) when the current push/pull started. Drives
   *  the "elapsed Ns" pill beside the spinner once an operation runs past
   *  ~2 seconds. Null when no operation is in flight. */
  workspacePushPullStartedAt: number | null;
  /** The most recent failed push or pull, remembered until the next
   *  transfer starts. The failure itself lives in a toast that disappears;
   *  this is what keeps it findable — the sidebar's Devices dot turns
   *  amber and names it. */
  workspacePushPullError: { title: string; at: number } | null;
  /** Workspace the user just selected, before the backend snapshot that
   *  confirms it has round-tripped. Written synchronously in the click's own
   *  task so the sidebar highlight and the target pane paint without waiting
   *  on IPC; the active-* selectors below prefer it over
   *  `appState.active_workspace_id` as long as the workspace exists in the
   *  snapshot we already hold. Cleared on confirmation (`setAppState`), on a
   *  rejected activate, or by the activation helper's safety timeout. */
  pendingActiveWorkspaceId: string | null;
  /** `Date.now()` when the pending selection was opened. Lets the activation
   *  helper age out a pending id the backend never confirmed. */
  pendingActivationAt: number | null;
  /** Highest revision already applied, from either a snapshot or a delta —
   *  the two share one counter. Snapshots at or below it are stale and
   *  dropped; deltas must be exactly one above it. See `setAppState` and
   *  `applyAppStateDelta`. */
  lastSeenRevision: number;
  /** The backend process `lastSeenRevision` was counted by, or `null` before
   *  the first stamped message.
   *
   *  The revision counter is process-lifetime and restarts at 0, so a revision
   *  number only means anything next to another from the SAME backend. That is
   *  invisible on the desktop (the webview dies with the process) and fatal on
   *  web remote, where the page outlives a backend restart: every post-restart
   *  snapshot, delta and heartbeat carried a number below the old
   *  `lastSeenRevision`, so the ordering guards discarded all of them and the
   *  page froze until the new counter climbed past the old one. Comparing
   *  tokens first makes "the counter restarted" distinguishable from "this
   *  message is stale". */
  backendInstance: string | null;
  /** True between "we noticed we are behind" and the full snapshot landing.
   *  Makes the resync single-flight. Deltas arriving inside the window are
   *  buffered, not dropped: the snapshot is a baseline, and anything stamped
   *  above it still has to be applied on top. */
  resyncInFlight: boolean;
  /** Bumped every time a resync is opened. `use-app-state.ts` watches it and
   *  performs the actual `get_app_state` fetch — the store stays free of IPC. */
  resyncRequestId: number;
  /** Deltas that cannot be applied yet, keyed by revision: they arrived ahead
   *  of their turn, or during a resync whose baseline has not landed. Capped
   *  at [`DELTA_BUFFER_LIMIT`]. */
  deltaBuffer: Map<number, AppStateDelta>;
  /** Identifies the open reorder window, or 0 when none is open.
   *  `use-app-state.ts` watches it and arms one [`DELTA_REORDER_WINDOW_MS`]
   *  timer per window; the id lets a late timer recognise that its window has
   *  already been resolved. */
  gapWindowId: number;
  /** The reorder window elapsed without the missing revision arriving, so the
   *  gap is real and only a full snapshot can close it. No-op when the window
   *  named by `windowId` has already been resolved or superseded. */
  expireGapWindow: (windowId: number) => void;
  setAppState: (snapshot: AppStateSnapshot) => void;
  /** Apply one domain delta at `revision`. Immutable and targeted: every
   *  object the delta doesn't name keeps its previous reference, which is the
   *  entire point of the delta path. */
  applyAppStateDelta: (
    revision: number,
    delta: AppStateDelta,
    instance?: string,
  ) => void;
  /** Open a single-flight full resync (no-op while one is already open). */
  requestResync: () => void;
  /** Close the resync window — called once the fetch settles, success or
   *  failure, so a rejected fetch can't wedge the delta path shut. */
  endResync: () => void;
  setHomeDir: (homeDir: string) => void;
  setWorkspacePushPullInFlight: (workspaceId: string | null) => void;
  /** Record a failed transfer for the Devices indicator (null clears). */
  setWorkspacePushPullError: (title: string | null) => void;
  beginPendingActivation: (workspaceId: string) => void;
  /** Drop the optimistic selection. Pass the id the caller opened to make the
   *  clear conditional — a late rollback or timeout must not cancel a newer
   *  selection the user has since made. */
  clearPendingActivation: (workspaceId?: string) => void;
}

/**
 * Apply one domain delta to a snapshot, immutably and narrowly.
 *
 * The contract that makes deltas worth having: every object the delta does
 * NOT name keeps its previous reference. Only the changed workspace (not the
 * other 78), only the changed map, and the snapshot wrapper itself are new —
 * so `useShallow`/primitive selectors and `React.memo` boundaries on the rows
 * all short-circuit for the untouched majority.
 *
 * Returns the input snapshot unchanged when the delta is a no-op (the backend
 * change-gates too, but a re-applied delta must not manufacture a re-render).
 * Pure — exported for unit tests.
 */
export function applyDeltaToSnapshot(
  snapshot: AppStateSnapshot,
  delta: AppStateDelta,
  revision: number,
): AppStateSnapshot {
  switch (delta.domain) {
    case "workspace_git": {
      const index = snapshot.workspaces.findIndex(
        (w) => w.workspace_id === delta.workspace_id,
      );
      if (index === -1) return snapshot;
      const workspace = snapshot.workspaces[index];
      const git = delta.git;
      const unchanged =
        (workspace.is_git ?? true) === git.is_git &&
        workspace.git_branch === git.git_branch &&
        workspace.git_ahead === git.git_ahead &&
        workspace.git_behind === git.git_behind &&
        workspace.git_additions === git.git_additions &&
        workspace.git_deletions === git.git_deletions &&
        workspace.git_changed_files === git.git_changed_files;
      if (unchanged) return snapshot;
      const workspaces = snapshot.workspaces.slice();
      // Field names are shared with the snapshot, so the spread IS the patch.
      // Nested `surfaces`/`tabs` come along by reference — untouched.
      workspaces[index] = { ...workspace, ...git };
      return { ...snapshot, workspaces, snapshot_revision: revision };
    }
    case "detected_ports":
      // The backend change-gates this domain, so a deep compare here would
      // only duplicate that gate; the identity check covers re-application.
      if (snapshot.detected_ports === delta.ports) return snapshot;
      return { ...snapshot, detected_ports: delta.ports, snapshot_revision: revision };
    case "pane_status": {
      const current = snapshot.pane_statuses[delta.pane_id];
      if (delta.status === null) {
        if (current === undefined) return snapshot;
        const pane_statuses = { ...snapshot.pane_statuses };
        delete pane_statuses[delta.pane_id];
        return { ...snapshot, pane_statuses, snapshot_revision: revision };
      }
      if (current === delta.status) return snapshot;
      return {
        ...snapshot,
        pane_statuses: { ...snapshot.pane_statuses, [delta.pane_id]: delta.status },
        snapshot_revision: revision,
      };
    }
  }
}

/**
 * How long a delta that arrived ahead of its turn waits for the revisions
 * before it. The backend stamps under the state lock but emits after releasing
 * it, so a big snapshot's serialize window can let a later-stamped delta reach
 * the renderer first. That is a reordering, not a loss — resyncing on it would
 * trade a few hundred buffered bytes for a full snapshot fetch.
 */
export const DELTA_REORDER_WINDOW_MS = 100;

/**
 * Ceiling on buffered deltas. Reordering is a sub-frame effect, so a buffer
 * this deep already means something other than reordering is wrong; past it a
 * full resync is both cheaper and more certain than holding more state.
 */
export const DELTA_BUFFER_LIMIT = 64;

/** Shared empty buffer so "nothing pending" keeps one stable reference. */
const EMPTY_DELTA_BUFFER: Map<number, AppStateDelta> = new Map();

/** Window ids only ever need to be distinguishable, never meaningful. */
let gapWindowSeq = 0;

/**
 * Apply every buffered delta that continues the sequence from
 * `lastSeenRevision`, in revision order, and report what is left.
 *
 * Contiguity is deliberate. `lastSeenRevision` means "everything up to here has
 * been applied"; skipping a revision because its neighbours happen to be
 * buffered would break that invariant silently — the domains are absolute
 * writes, but a skipped revision may belong to a *different* domain, and
 * nothing would ever notice it was missed. Leftovers keep the window open
 * instead, which converges on a resync.
 *
 * Pure — exported for unit tests.
 */
export function drainDeltaBuffer(
  appState: AppStateSnapshot,
  lastSeenRevision: number,
  buffer: Map<number, AppStateDelta>,
): {
  appState: AppStateSnapshot;
  lastSeenRevision: number;
  buffer: Map<number, AppStateDelta>;
} {
  if (buffer.size === 0) return { appState, lastSeenRevision, buffer };
  let next = appState;
  let revision = lastSeenRevision;
  let remaining: Map<number, AppStateDelta> | null = null;
  for (;;) {
    const delta = buffer.get(revision + 1);
    if (delta === undefined) break;
    remaining ??= new Map(buffer);
    revision += 1;
    remaining.delete(revision);
    next = applyDeltaToSnapshot(next, delta, revision);
  }
  // Everything at or below the new baseline is superseded, buffered or not.
  if (remaining === null && hasStaleEntries(buffer, revision)) remaining = new Map(buffer);
  if (remaining !== null) {
    for (const buffered of remaining.keys()) {
      if (buffered <= revision) remaining.delete(buffered);
    }
  }
  const settled = remaining ?? buffer;
  return {
    appState: next,
    lastSeenRevision: revision,
    buffer: settled.size === 0 ? EMPTY_DELTA_BUFFER : settled,
  };
}

/**
 * Hold a delta that cannot be applied yet, opening a reorder window if none is
 * open. Past [`DELTA_BUFFER_LIMIT`] the buffer is abandoned for a resync — the
 * snapshot is a complete baseline, so no correctness is lost by forgetting
 * what was pending.
 */
function bufferDelta(
  state: Pick<
    AppStore,
    "deltaBuffer" | "gapWindowId" | "resyncInFlight" | "resyncRequestId"
  >,
  revision: number,
  delta: AppStateDelta,
): Partial<AppStore> {
  if (state.deltaBuffer.size >= DELTA_BUFFER_LIMIT) {
    return {
      deltaBuffer: EMPTY_DELTA_BUFFER,
      gapWindowId: 0,
      ...(state.resyncInFlight
        ? null
        : { resyncInFlight: true, resyncRequestId: state.resyncRequestId + 1 }),
    };
  }
  const deltaBuffer = new Map(state.deltaBuffer);
  deltaBuffer.set(revision, delta);
  return {
    deltaBuffer,
    // While a resync is in flight the snapshot resolves the sequence, so no
    // timer is needed (and a second resync behind it would be wasted work).
    gapWindowId: state.resyncInFlight
      ? 0
      : state.gapWindowId !== 0
        ? state.gapWindowId
        : ++gapWindowSeq,
  };
}

/**
 * Whether `instance` names a backend process other than the one our revision
 * baseline was counted by.
 *
 * An empty / missing token is "unstamped" — a restored layout, a mock, or an
 * older backend — and never invalidates the baseline: those messages carry
 * revision 0 and are applied unconditionally anyway.
 *
 * The token is compared for EQUALITY only, never ordered. It doesn't need to
 * be: messages from a dead backend cannot arrive after the reconnect, because
 * they rode a socket that died with it. So "different" always means "newer".
 */
function isRestartedBackend(
  instance: string | undefined,
  current: string | null,
): boolean {
  if (!instance) return false;
  return current !== null && current !== instance;
}

function hasStaleEntries(buffer: Map<number, AppStateDelta>, revision: number): boolean {
  for (const buffered of buffer.keys()) {
    if (buffered <= revision) return true;
  }
  return false;
}

export const useAppStore = create<AppStore>((set) => ({
  appState: null,
  homeDir: null,
  workspacePushPullInFlight: null,
  workspacePushPullStartedAt: null,
  workspacePushPullError: null,
  pendingActiveWorkspaceId: null,
  pendingActivationAt: null,
  lastSeenRevision: 0,
  backendInstance: null,
  resyncInFlight: false,
  resyncRequestId: 0,
  deltaBuffer: EMPTY_DELTA_BUFFER,
  gapWindowId: 0,
  // Structural sharing: reconcile the incoming snapshot against the one we
  // already hold so unchanged subtrees keep their previous reference. When
  // nothing changed the returned ref equals the previous snapshot, so zustand
  // selector subscribers see zero fan-out. See `@/lib/structural-share`.
  setAppState: (snapshot) =>
    set((state) => {
      const revision = snapshot.snapshot_revision ?? 0;
      // Restart guard, BEFORE the ordering guard. A different instance token
      // means the counter this snapshot was stamped by is not the one our
      // baseline came from, so comparing the two numbers is meaningless — and
      // comparing them anyway is exactly what froze a web-remote page across a
      // backend restart: the fresh snapshot's low revision read as "stale" and
      // was dropped, forever. A restart makes this snapshot the new baseline,
      // so the revision floor drops to 0 and any delta buffered against the
      // old counter is discarded (its missing revisions belong to a process
      // that no longer exists and will never arrive).
      const restarted = isRestartedBackend(
        snapshot.snapshot_instance,
        state.backendInstance,
      );
      if (restarted) {
        state = {
          ...state,
          lastSeenRevision: 0,
          deltaBuffer: EMPTY_DELTA_BUFFER,
          gapWindowId: 0,
        };
      }
      // Ordering guard. The backend stamps every emitted snapshot with a
      // strictly increasing `snapshot_revision`; a snapshot at or below the
      // one we already applied lost the race (a background git/PR/port emit
      // built before the activation emit but delivered after it) and would
      // otherwise stomp the freshly selected workspace back to the old one.
      // Revision 0 means "unrevisioned" — restored state, an older backend,
      // or a mock — and is always applied.
      if (revision > 0 && revision <= state.lastSeenRevision) return state;

      const shared = state.appState
        ? shareStructural(state.appState, snapshot)
        : snapshot;
      // Reconcile: the backend agrees with the optimistic selection, so the
      // pending id has no further work to do.
      const confirmed =
        state.pendingActiveWorkspaceId !== null &&
        shared.active_workspace_id === state.pendingActiveWorkspaceId;

      // This snapshot is a full baseline, so it closes any reorder gap it
      // covers: buffered deltas at or below its revision are superseded, and
      // the ones above it are replayed on top in order.
      const baseline = revision > 0 ? revision : state.lastSeenRevision;
      const drained = drainDeltaBuffer(shared, baseline, state.deltaBuffer);

      return {
        appState: drained.appState,
        lastSeenRevision: drained.lastSeenRevision,
        deltaBuffer: drained.buffer,
        // Adopt whatever instance stamped this snapshot. Only a stamped
        // snapshot moves it — an unstamped one (restored layout, mock) says
        // nothing about which backend is live.
        ...(snapshot.snapshot_instance
          ? { backendInstance: snapshot.snapshot_instance }
          : null),
        gapWindowId:
          drained.buffer.size === 0
            ? 0
            : state.gapWindowId !== 0
              ? state.gapWindowId
              : ++gapWindowSeq,
        ...(confirmed
          ? { pendingActiveWorkspaceId: null, pendingActivationAt: null }
          : null),
      };
    }),
  // Deltas are small and already ordered, so they skip the emit-coalescing
  // window entirely and land in the click's own frame budget.
  applyAppStateDelta: (revision, delta, instance) =>
    set((state) => {
      // No baseline to apply onto — the boot fetch is what establishes one.
      if (state.appState === null) return state;
      // A delta from a RESTARTED backend has nothing valid to patch: our
      // snapshot describes the previous process's world, and this delta's
      // revision counts from a fresh 0. Patching one onto the other would
      // produce a snapshot that never existed. Drop it, drop the buffer of
      // deltas awaiting revisions the dead process will never emit, and pull a
      // real baseline. (The reseeded snapshot normally beats the first delta
      // here; this is the case where it doesn't.)
      if (isRestartedBackend(instance, state.backendInstance)) {
        return {
          lastSeenRevision: 0,
          deltaBuffer: EMPTY_DELTA_BUFFER,
          gapWindowId: 0,
          ...(state.resyncInFlight
            ? null
            : {
                resyncInFlight: true,
                resyncRequestId: state.resyncRequestId + 1,
              }),
        };
      }
      if (revision <= state.lastSeenRevision) return state; // lost the race
      // A full snapshot is on its way and will be the baseline. Buffer rather
      // than drop: this delta may be stamped ABOVE the revision that snapshot
      // reports, and dropping it would leave that change invisible until some
      // later emit happened to carry it.
      if (state.resyncInFlight) return bufferDelta(state, revision, delta);
      if (revision !== state.lastSeenRevision + 1) {
        // Forward gap. The backend stamps under the state lock but emits after
        // releasing it, so a delta stamped later can overtake a snapshot still
        // being serialized — far more likely than a lost message. Hold it for
        // one short window and let the missing revisions catch up.
        return bufferDelta(state, revision, delta);
      }
      // No delta domain carries `active_workspace_id`, so an in-flight
      // optimistic selection is untouched here by construction.
      const applied = applyDeltaToSnapshot(state.appState, delta, revision);
      const drained = drainDeltaBuffer(applied, revision, state.deltaBuffer);
      return {
        appState: drained.appState,
        lastSeenRevision: drained.lastSeenRevision,
        deltaBuffer: drained.buffer,
        // The window only closes when nothing is left waiting on a revision.
        gapWindowId: drained.buffer.size === 0 ? 0 : state.gapWindowId,
      };
    }),
  expireGapWindow: (windowId) =>
    set((state) => {
      if (state.gapWindowId !== windowId) return state;
      // Keep the buffer: the snapshot reconciles it (superseded below its
      // revision, replayed above it).
      if (state.resyncInFlight) return { gapWindowId: 0 };
      return {
        gapWindowId: 0,
        resyncInFlight: true,
        resyncRequestId: state.resyncRequestId + 1,
      };
    }),
  requestResync: () =>
    set((state) =>
      state.resyncInFlight
        ? state
        : {
            resyncInFlight: true,
            resyncRequestId: state.resyncRequestId + 1,
            // The snapshot now owns gap resolution; the reorder timer would
            // only open a second resync behind it.
            gapWindowId: 0,
          },
    ),
  endResync: () =>
    set((state) => {
      if (!state.resyncInFlight) return state;
      // A rejected fetch closes the window without ever delivering a baseline,
      // so anything still buffered has nothing on its way to resolve it. Drain
      // what became contiguous and put the rest back under a reorder window,
      // which converges on another resync instead of stranding it.
      if (state.deltaBuffer.size === 0 || state.appState === null) {
        return { resyncInFlight: false };
      }
      const drained = drainDeltaBuffer(
        state.appState,
        state.lastSeenRevision,
        state.deltaBuffer,
      );
      return {
        resyncInFlight: false,
        appState: drained.appState,
        lastSeenRevision: drained.lastSeenRevision,
        deltaBuffer: drained.buffer,
        gapWindowId:
          drained.buffer.size === 0
            ? 0
            : state.gapWindowId !== 0
              ? state.gapWindowId
              : ++gapWindowSeq,
      };
    }),
  setHomeDir: (homeDir) => set({ homeDir }),
  beginPendingActivation: (workspaceId) =>
    set({
      pendingActiveWorkspaceId: workspaceId,
      pendingActivationAt: Date.now(),
    }),
  clearPendingActivation: (workspaceId) =>
    set((state) => {
      if (state.pendingActiveWorkspaceId === null) return state;
      if (workspaceId !== undefined && state.pendingActiveWorkspaceId !== workspaceId) {
        return state;
      }
      return { pendingActiveWorkspaceId: null, pendingActivationAt: null };
    }),
  // Pair the workspace id with a start timestamp so "elapsed" can
  // be computed at render time. Null clears both. Starting a transfer
  // also forgets the last failure: the user is retrying, and a stale
  // amber dot would outlive the problem it pointed at.
  setWorkspacePushPullInFlight: (workspaceId) =>
    set((state) => ({
      workspacePushPullInFlight: workspaceId,
      workspacePushPullStartedAt: workspaceId === null ? null : Date.now(),
      workspacePushPullError:
        workspaceId === null ? state.workspacePushPullError : null,
    })),
  setWorkspacePushPullError: (title) =>
    set({
      workspacePushPullError: title === null ? null : { title, at: Date.now() },
    }),
}));

// Derived selectors
//
// IMPORTANT — re-render economics:
//
// The Rust backend rebuilds the full `AppStateSnapshot` (workspaces,
// surfaces, panes, tabs) on every `emit_app_state` and ships it to the
// renderer over Tauri IPC. Every workspace object — and every nested
// surfaces/tabs array — therefore has a *fresh reference* on every
// backend tick. Agent token streaming, the 5-second git poll, hook
// events, and PR/port refreshes all fire emit_app_state, so under
// normal use these ticks happen many times per second.
//
// `useActiveWorkspace()` returns the matching workspace OBJECT from the
// snapshot. Before structural sharing, a fresh snapshot meant a fresh
// workspace reference every tick, so every consumer subscribed via this
// hook re-rendered on every tick — cascading into MarkdownRendered (full
// markdown re-parse), the pane container, the tab bar, etc.
//
// Mitigation:
//
// 0. `setAppState` (above) now runs `shareStructural(prev, next)` before
//    storing the snapshot. It reconciles the fresh IPC payload against the
//    snapshot we already hold and REUSES prev references for every subtree
//    that is deep-equal — so a no-op backend tick returns the *same*
//    top-level ref (zero selector fan-out), and a change to one workspace
//    leaves all other workspaces', surfaces', and pane subtrees' refs
//    untouched. This is what makes `useActiveWorkspace` / `useActiveSurface`
//    stable across no-op ticks, and it is what makes the `React.memo`
//    boundaries on the pane tree (added in the same change) actually pay off
//    — an unchanged pane subtree keeps its identity, so memo short-circuits.
//    (This is NOT the old full-snapshot JSON.stringify dedup, which was
//    removed for causing freezes — see `src/hooks/use-app-state.ts`.)
//
// 1. Primitive-slice selectors (`useActiveWorkspaceId`,
//    `useActiveWorkspaceCwd`, etc.) below return primitive strings
//    that compare with `===` and are stable across backend ticks
//    unless the slice itself changed. Lightweight consumers (title-bar
//    SearchTrigger, run-button, file-search dialog, content-search
//    dialog) use these instead of the full-object selector.
//
// 2. `useActiveWorkspace()` is kept for consumers that legitimately
//    need the full workspace object (WorkspaceMain). The heaviest cost
//    downstream of WorkspaceMain — react-markdown re-parsing — is
//    mitigated separately by `React.memo` on `MarkdownRendered`.
//
// Before structural sharing, `useShallow` would NOT have helped: the Rust
// snapshot rebuild gave every nested array (surfaces, tabs) fresh refs, so
// a shallow compare always tripped. Structural sharing fixes this at the
// source by restoring reference stability for unchanged subtrees.
/**
 * The workspace the UI should be showing: the optimistic pending selection
 * when there is one, otherwise whatever the last snapshot said.
 *
 * The pending id only wins while its workspace is present in the snapshot we
 * already hold — that is the whole optimistic-paint condition. All the data
 * the pane tree needs (surfaces, panes, session ids) is already in the
 * renderer, so the target can mount from local state in the click's own task;
 * a pending id we know nothing about (a freshly created workspace, a stale id
 * for a workspace that has since been closed) has nothing to paint and must
 * defer to the backend.
 *
 * Pure function of the store slice — safe inside a zustand selector, and
 * exported for unit tests.
 */
export function selectActiveWorkspaceId(s: {
  appState: AppStateSnapshot | null;
  pendingActiveWorkspaceId: string | null;
}): string | null {
  const pending = s.pendingActiveWorkspaceId;
  if (
    pending !== null &&
    s.appState?.workspaces.some((w) => w.workspace_id === pending)
  ) {
    return pending;
  }
  return s.appState?.active_workspace_id ?? null;
}

/** The workspace object for `selectActiveWorkspaceId`, or null. */
function selectActiveWorkspace(s: {
  appState: AppStateSnapshot | null;
  pendingActiveWorkspaceId: string | null;
}): WorkspaceSnapshot | null {
  if (!s.appState) return null;
  const id = selectActiveWorkspaceId(s);
  if (id === null) return null;
  return s.appState.workspaces.find((w) => w.workspace_id === id) ?? null;
}

export function useActiveWorkspace(): WorkspaceSnapshot | null {
  return useAppStore(selectActiveWorkspace);
}

/** Primitive: the active workspace's id, or null when no workspace is
 *  active. Stable across backend ticks unless the active workspace
 *  actually changes. Use this in components that only need the id. */
export function useActiveWorkspaceId(): string | null {
  return useAppStore(selectActiveWorkspaceId);
}

/** Primitive: the active workspace's cwd, or null. Used by search
 *  dialogs that need a path, not the whole workspace. Returns the
 *  primitive string so subscribers don't churn on full-snapshot
 *  rebuilds. */
export function useActiveWorkspaceCwd(): string | null {
  return useAppStore((s) => selectActiveWorkspace(s)?.cwd ?? null);
}

/** Primitive: the active workspace's project_root, or null. */
export function useActiveWorkspaceProjectRoot(): string | null {
  return useAppStore((s) => selectActiveWorkspace(s)?.project_root ?? null);
}

/** Primitive: the active workspace's git branch, or null. Used for
 *  the search-trigger label in the title bar. */
export function useActiveWorkspaceBranch(): string | null {
  return useAppStore((s) => selectActiveWorkspace(s)?.git_branch ?? null);
}

/** Subscribe to the cached home directory. Null until the initial
 *  `getHomeDir()` Tauri call resolves at App mount. */
export function useHomeDir(): string | null {
  return useAppStore((s) => s.homeDir);
}

/** Recursively walk a pane tree checking whether the given pane id is
 *  present anywhere under the node. */
function paneTreeContains(node: PaneNodeSnapshot, paneId: string): boolean {
  if (node.pane_id === paneId) return true;
  if (node.kind === "split") {
    return node.children.some((child) => paneTreeContains(child, paneId));
  }
  return false;
}

/** Recursively collect every terminal pane's `session_id` under the given
 *  node into the supplied workspace map. Mutates `out` in place to avoid
 *  intermediate Map allocations during the walk. */
function collectTerminalSessions(
  node: PaneNodeSnapshot,
  workspaceId: string,
  out: Map<string, string>,
): void {
  if (node.kind === "terminal") {
    out.set(node.session_id, workspaceId);
    return;
  }
  if (node.kind === "split") {
    for (const child of node.children) {
      collectTerminalSessions(child, workspaceId, out);
    }
  }
  // browser / agent_chat panes have no session_id — skip.
}

/** Build a reverse index from `session_id` to the `workspace_id` that owns
 *  the terminal pane carrying that session.
 *
 *  Replaces the O(panes^2) `JSON.stringify(surf.root).includes(sid)` scan
 *  used by the terminal scrollback save / restore paths. With many
 *  workspaces, that pattern stringified every surface tree once per
 *  terminal mount and once per scrollback save — wasteful at scale. The
 *  index is built once per `AppStateSnapshot` reference (see
 *  `getSessionWorkspaceId` for caching) so repeated lookups are O(1).
 *
 *  Pure function of the snapshot — exported for unit testing.
 */
export function buildSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  const index = new Map<string, string>();
  if (!appState) return index;
  for (const ws of appState.workspaces) {
    for (const surface of ws.surfaces) {
      collectTerminalSessions(surface.root, ws.workspace_id, index);
    }
  }
  return index;
}

// WeakMap caches the index per snapshot reference. The Rust-side state
// rebuilds the snapshot on every change, so a new reference always means a
// stale index — and a re-used reference means the index is still correct.
const sessionWorkspaceIndexCache = new WeakMap<
  AppStateSnapshot,
  Map<string, string>
>();

function getCachedSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  if (!appState) return new Map();
  const cached = sessionWorkspaceIndexCache.get(appState);
  if (cached) return cached;
  const fresh = buildSessionWorkspaceIndex(appState);
  sessionWorkspaceIndexCache.set(appState, fresh);
  return fresh;
}

/** Imperative lookup: workspace id for a given terminal session, or null
 *  if no terminal pane currently owns it. Reads the cached index from
 *  `useAppStore.getState()` so callers inside effects / async IIFEs do not
 *  pay the build cost on every call. Returns null (not "") so call sites
 *  can apply their own fallback. */
export function getSessionWorkspaceId(sessionId: string): string | null {
  const { appState } = useAppStore.getState();
  const index = getCachedSessionWorkspaceIndex(appState);
  return index.get(sessionId) ?? null;
}

/** React hook variant — subscribes to `appState` and returns the cached
 *  index. Re-renders only when the snapshot reference changes (which is
 *  also when the index would actually differ). */
export function useSessionWorkspaceIndex(): Map<string, string> {
  return useAppStore((s) => getCachedSessionWorkspaceIndex(s.appState));
}

/** The cwd of the workspace owning a given terminal session, or null.
 *
 *  Used by the terminal pane header to decide whether a session's live
 *  directory is worth showing: at the workspace root it says nothing.
 *  Resolves through the session→workspace index rather than assuming the
 *  active workspace, so it stays correct for any pane tree that renders
 *  outside the active-workspace path. Returns a primitive so subscribers
 *  don't churn on unrelated snapshot rebuilds. */
export function useWorkspaceCwdForSession(sessionId: string): string | null {
  return useAppStore((s) => {
    if (!s.appState) return null;
    const workspaceId = getCachedSessionWorkspaceIndex(s.appState).get(
      sessionId,
    );
    if (!workspaceId) return null;
    return (
      s.appState.workspaces.find((w) => w.workspace_id === workspaceId)?.cwd ??
      null
    );
  });
}

/** Find the workspace id that owns a given pane, if any.
 *
 *  Used by `AgentChatPane`'s draft-aware mount guard: when the pane's
 *  own `thread_id` field hasn't arrived yet (the Stage C race between
 *  `agent_chat_start_session`'s state emit and the pane mount), we
 *  need to know which workspace the pane belongs to so we can look up
 *  the promoted draft. Pure function of the snapshot — safe to call
 *  from inside a Zustand selector.
 */
export function findWorkspaceIdForPane(
  state: { appState: AppStateSnapshot | null },
  paneId: string,
): string | null {
  if (!state.appState) return null;
  for (const ws of state.appState.workspaces) {
    for (const surface of ws.surfaces) {
      if (paneTreeContains(surface.root, paneId)) {
        return ws.workspace_id;
      }
    }
  }
  return null;
}

export function useActiveSurface(): SurfaceSnapshot | null {
  return useAppStore((s) => {
    const ws = selectActiveWorkspace(s);
    if (!ws) return null;
    return ws.surfaces.find((sf) => sf.surface_id === ws.active_surface_id) ?? null;
  });
}

// Project grouping — groups unsorted workspaces by their project root directory

export interface ProjectGroup {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
}

/** Derive the project root for grouping: `project_root`, `cwd` fallback.
 *  Accepts anything carrying the two fields (live `WorkspaceSnapshot`s
 *  and `ArchivedWorkspaceSnapshot`s alike) so every surface groups by
 *  the same key. */
export function resolveProjectRoot(ws: {
  project_root?: string | null;
  cwd: string;
}): string {
  return ws.project_root || ws.cwd;
}

/**
 * The single label rule for a project root: the dedicated "Home" label
 * when the root IS the user's home directory, the path basename
 * otherwise. Shared by the sidebar grouping and Settings → Archive so
 * both surfaces name a project identically. (Duplicate-basename
 * disambiguation is a collision post-pass in `groupWorkspacesByProject`,
 * layered on top of this rule.)
 */
export function projectDisplayName(
  projectPath: string,
  homeDir: string | null,
): string {
  return homeDir !== null && projectPath === homeDir
    ? "Home"
    : basename(projectPath);
}

/**
 * Pure grouping — exported so unit tests can exercise the logic
 * without rendering a hook. `useProjectGroupedWorkspaces` is a thin
 * `useMemo` wrapper.
 *
 * When `homeDir` is non-null, workspaces whose resolved project root
 * equals `homeDir` land in a dedicated group labelled "Home" (instead
 * of e.g. "user" or "home/user" from the path-basename rule). When
 * `homeDir` is null, grouping falls back to today's path-only rule.
 *
 * Under the Step 5 Home rework (Stage B+), `sidebar-workspace-list`
 * stops filtering out `workspace_type === "home"` workspaces, so this
 * label rule is what surfaces them correctly in the sidebar.
 */
/**
 * The single host a group's workspaces live on, or `null` when they're
 * local. Returns `undefined` when the group is empty or its workspaces
 * straddle multiple hosts — in that case the host can't disambiguate
 * the group and we fall back to path tails.
 */
function groupHostId(workspaces: WorkspaceSnapshot[]): number | null | undefined {
  let hostId: number | null | undefined = undefined;
  for (const ws of workspaces) {
    const h = ws.host_id ?? null;
    if (hostId === undefined) hostId = h;
    else if (hostId !== h) return undefined; // mixed → ambiguous
  }
  return hostId;
}

export function groupWorkspacesByProject(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
  hostNameById?: ReadonlyMap<number, string> | null,
): ProjectGroup[] {
  const groups = new Map<string, { name: string; path: string; workspaces: WorkspaceSnapshot[] }>();

  for (const ws of workspaces) {
    const projectPath = resolveProjectRoot(ws);
    const projectName = projectDisplayName(projectPath, homeDir);

    if (!groups.has(projectPath)) {
      groups.set(projectPath, { name: projectName, path: projectPath, workspaces: [] });
    }
    groups.get(projectPath)!.workspaces.push(ws);
  }

  const result = Array.from(groups.values()).map((g) => ({
    projectName: g.name,
    projectPath: g.path,
    workspaces: g.workspaces,
  }));

  // Disambiguate duplicate project names. Two project roots can share a
  // basename when the same repo lives on different machines (e.g. a
  // local `~/projects/app` and the same `app` on a remote host) or in
  // sibling directories. We prefer the host as the distinguishing tag:
  // the local copy keeps its clean basename and each remote copy is
  // suffixed with " · <host>". When the host can't tell the copies
  // apart (both local, both on the same host, or host names
  // unavailable) we fall back to appending just enough trailing path
  // segments to make every colliding label unique. The "Home" group is
  // always unique (single homeDir path), so this never rewrites it.
  const byName = new Map<string, typeof result>();
  for (const g of result) {
    const list = byName.get(g.projectName);
    if (list) list.push(g);
    else byName.set(g.projectName, [g]);
  }

  for (const colliding of byName.values()) {
    if (colliding.length < 2) continue;

    // Pass 1 — tag remote groups with their host name; locals (and
    // groups whose host we can't name) keep the clean basename.
    for (const g of colliding) {
      const hostId = groupHostId(g.workspaces);
      const hostName = hostId != null ? hostNameById?.get(hostId) ?? null : null;
      if (hostName) g.projectName = `${g.projectName} · ${hostName}`;
    }

    const unique = (gs: typeof colliding) =>
      new Set(gs.map((g) => g.projectName)).size === gs.length;
    if (unique(colliding)) continue;

    // Pass 2 — fall back to trailing path segments, growing the tail
    // until every label is unique. Group keys are distinct absolute
    // paths, so this always converges by the longest path's depth.
    const maxSegments = Math.max(...colliding.map((g) => segmentCount(g.projectPath)));
    for (let n = 2; n <= maxSegments; n++) {
      const tails = colliding.map((g) => tailSegments(g.projectPath, n));
      if (new Set(tails).size === colliding.length) {
        colliding.forEach((g, i) => {
          if (tails[i].includes("/")) g.projectName = tails[i];
        });
        break;
      }
    }
  }

  return result;
}

export function useProjectGroupedWorkspaces(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
  hosts?: HostView[],
): ProjectGroup[] {
  return useMemo(() => {
    const hostNameById = hosts
      ? new Map(hosts.map((h) => [h.id, h.name] as const))
      : null;
    return groupWorkspacesByProject(workspaces, homeDir, hostNameById);
  }, [workspaces, homeDir, hosts]);
}
