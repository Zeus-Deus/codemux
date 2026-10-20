import { useEffect, useMemo, useRef, useState } from "react";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import {
  selectSidebarAutoSettleDays,
  useSettingsStore,
} from "@/stores/settings-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { prSetSummary, prsDescribeThisCheckout, workspacePrs } from "@/lib/workspace-prs";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import { emitInboxLifecycle as emit } from "./inbox-lifecycle-events";
import {
  clampTimerDelay,
  effectiveActivityAt,
  shouldAutoSettle,
} from "./sidebar-inbox";

/** Wake a snoozed workspace back into the active list. A wake still runs
 *  while a workspace is pinned: the pin overrides where the card is *shown*,
 *  not whether its snooze ticket comes due, so the preserved shelf entry is
 *  cleared on schedule and unpinning later reveals an honest active card
 *  rather than an expired snooze. */
function wake(
  ws: WorkspaceSnapshot | undefined,
  workspaceId: string,
  reason: "activity" | "timer",
): void {
  const wasPinned = ws?.pinned_at != null;
  useSidebarInboxStore.getState().unsnooze(workspaceId, reason);
  emit({ kind: "woke", workspaceId, reason, wasPinned });
}

/** The inbox's durable lifecycle: every effect that persists settled /
 *  snoozed / keep-active / activity state in response to pane statuses or the
 *  clock.
 *
 *  It lives outside both sidebar views on purpose. Only the selected view is
 *  mounted, so when these effects belonged to the Activity view a user who
 *  stayed in Projects silently missed every transition — a settled workspace
 *  that started working was never resurfaced, and once it finished it went
 *  straight back under "Show N settled" with nothing able to recover it.
 *  Mount this exactly once, somewhere that is always rendered
 *  (`SidebarWorkspaceList` does it via `<InboxLifecycle />`). Views that want
 *  to animate a transition listen through `subscribeInboxLifecycle`. */
export function useInboxLifecycle(): void {
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses);
  const appStateLoaded = useAppStore((s) => s.appState !== null);
  const allWorkspaces = useMemo(() => workspaces ?? [], [workspaces]);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const selectedWorkspaceId = useAppStore(selectActiveWorkspaceId) ?? "";
  const activeWorkspaceId = activeDraftId ? "" : selectedWorkspaceId;

  const autoSettleDays = useSettingsStore(selectSidebarAutoSettleDays);

  const load = useSidebarInboxStore((s) => s.load);
  const loaded = useSidebarInboxStore((s) => s.loaded);
  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const keepActive = useSidebarInboxStore((s) => s.keepActive);
  const pinOverrides = useSidebarInboxStore((s) => s.pinOverrides);
  const activity = useSidebarInboxStore((s) => s.activity);
  const prune = useSidebarInboxStore((s) => s.prune);

  // The tick the activity + auto-settle + wake effects re-run on. Shares the
  // one coarse interval with every elapsed label in the sidebar.
  const now = useCoarseClock(true);

  useEffect(() => {
    void load();
  }, [load]);

  // Drop settled entries whose workspace vanished (archived / deleted).
  useEffect(() => {
    if (!loaded || !appStateLoaded) return;
    prune(new Set(allWorkspaces.map((w) => w.workspace_id)));
  }, [loaded, appStateLoaded, allWorkspaces, prune]);

  // Last status observed by the activity effect. A keep-active pin is cleared
  // only by a NEW non-idle status edge, matching the server-side
  // "real activity clears the override" lifecycle. Re-reading an existing
  // review status after the user clicks Un-settle must not immediately erase
  // their decision and throw a merged PR back under the divider.
  const previousStatusRef = useRef(new Map<string, ActivePaneStatus | null>());

  // The workspace list as of the last commit, for the wake sweep's pin check
  // (it must not re-run on every workspace tick just to read `pinned_at`).
  // Declared before the lifecycle effects so it is current when they run.
  const workspacesRef = useRef(allWorkspaces);
  useEffect(() => {
    workspacesRef.current = allWorkspaces;
  });

  // Settle safety net: a settled workspace whose agent goes live ("working")
  // or blocked ("permission") resurfaces into the active list automatically,
  // so live or blocked work can never stay buried under the divider. Finished
  // ("review"), monitoring and idle settled rows stay put — only fresh
  // activity resurfaces them, and a background watch loop is not news: the
  // user parked the workspace *because* it was going to keep ticking. Unsettle
  // removes the entry from `settled`, so re-runs converge; we iterate a
  // snapshot and skip ids that aren't currently settled.
  //
  // Un-settle uses reason "activity" (not a user gesture), which clears any
  // keep-active pin.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    for (const entry of settled) {
      const ws = allWorkspaces.find((w) => w.workspace_id === entry.id);
      if (!ws) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status === "working" || status === "permission") {
        useSidebarInboxStore.getState().unsettle(entry.id, "activity");
        emit({ kind: "resurfaced", workspaceId: entry.id });
      }
    }
  }, [loaded, paneStatuses, settled, allWorkspaces]);

  // Snooze safety net (the hand-raise): a snoozed workspace whose agent starts
  // working or blocks on a permission prompt wakes immediately, wake time or
  // not. A snooze defers *waiting*; it can never be allowed to hide an agent
  // that is stuck asking the user a question, or the user would sit staring at
  // an empty inbox while work blocks behind a shelf they collapsed yesterday.
  //
  // No "is this newer than the snooze?" check is needed. `isSnoozeable`
  // refuses to snooze a working/blocked workspace in the first place, and
  // pane statuses are runtime-only (the backend clears them before persisting,
  // so no stale status survives a restart) — so reaching working/permission
  // here is always a transition that happened after the snooze was set.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    for (const entry of snoozed) {
      const ws = allWorkspaces.find((w) => w.workspace_id === entry.id);
      if (!ws) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status === "working" || status === "permission") {
        wake(ws, entry.id, "activity");
      }
    }
  }, [loaded, paneStatuses, snoozed, allWorkspaces]);

  // A precise timer at the soonest boundary, because the coarse clock alone
  // would make a wake up to a full tick late — a "In 1 hour" snooze returning
  // at 1h00m29s reads as a broken promise. Re-armed via the nonce after each
  // fire so a queue of wakes is walked one boundary at a time.
  const [wakeNonce, setWakeNonce] = useState(0);

  // Wake sweep: anything whose return ticket has come due goes back to the
  // active list. It reads the wall clock rather than the coarse one — the
  // boundary timer below re-runs this effect at the exact moment a ticket
  // comes due, and the coarse value can be a whole tick behind. The coarse
  // clock stays in the deps as the periodic backstop.
  useEffect(() => {
    if (!loaded) return;
    const nowMs = Date.now();
    for (const entry of snoozed) {
      if (nowMs < entry.until) continue;
      const ws = workspacesRef.current.find((w) => w.workspace_id === entry.id);
      wake(ws, entry.id, "timer");
    }
  }, [loaded, snoozed, now, wakeNonce]);

  useEffect(() => {
    if (!loaded || snoozed.length === 0) return;
    let soonest = Infinity;
    for (const entry of snoozed) soonest = Math.min(soonest, entry.until);
    if (!Number.isFinite(soonest)) return;
    const timer = window.setTimeout(
      () => setWakeNonce((n) => n + 1),
      clampTimerDelay(soonest - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [loaded, snoozed, wakeNonce]);

  // Activity observation: keep a client-side "last active" stamp per workspace
  // so the inactivity auto-settle rule has something to measure against. We
  // stamp when the agent is doing something (non-null status), when it's the
  // focused workspace, or when we've never recorded it before.
  //
  // The first-seen baseline prefers the backend's `last_active_at`: stamping
  // `Date.now()` there would throw away real idleness and restart every
  // workspace's clock on every app update, so a machine full of month-old work
  // would look brand new. Only when the backend has no stamp at all do we fall
  // back to now — that keeps the anti-mass-settle property (a workspace with no
  // usable history must never auto-settle the instant it is first seen).
  //
  // Only a NEW non-null status edge counts as genuine activity for pin
  // clearing. A repeated `review` snapshot is completed work, not a new run;
  // clearing on every non-null observation would make an explicitly
  // un-settled merged workspace settle again on the next render.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const noteActivity = useSidebarInboxStore.getState().noteActivity;
    const at = Date.now();
    const presentIds = new Set<string>();
    for (const ws of allWorkspaces) {
      presentIds.add(ws.workspace_id);
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      const hadPrevious = previousStatusRef.current.has(ws.workspace_id);
      const previousStatus = previousStatusRef.current.get(ws.workspace_id);
      const newAgentActivity =
        status !== null && hadPrevious && previousStatus !== status;
      previousStatusRef.current.set(ws.workspace_id, status);
      const isActiveWs = ws.workspace_id === activeWorkspaceId;
      const unseen = activity[ws.workspace_id] === undefined;
      if (status === null && !isActiveWs && !unseen) continue;
      // A live agent, or a workspace the user is looking at, is active *now*.
      // Anything else here is a first sighting, which inherits real history.
      const stamp =
        status !== null || isActiveWs ? at : ws.last_active_at ?? at;
      noteActivity(ws.workspace_id, stamp, { clearPin: newAgentActivity });
    }
    for (const id of previousStatusRef.current.keys()) {
      if (!presentIds.has(id)) previousStatusRef.current.delete(id);
    }
  }, [loaded, paneStatuses, allWorkspaces, activeWorkspaceId, activity, now]);

  // Auto-settle: server-side completion semantics. A merged/closed PR settles
  // immediately once the workspace is not running or blocked; otherwise
  // inactivity past the user's configured window is the fallback. No
  // leaving animation (this is a background lifecycle transition, not a
  // gesture) and deliberately no forward navigation: a sweep must never move
  // the user.
  //
  // Anti-fight invariants — the inbox has three base lifecycles (active /
  // settled / snoozed), plus the durable workspace-pin visibility override
  // and the local keep-active auto-settle override. These guards make
  // oscillation impossible:
  //   • auto-settle refuses working/permission, but completed review is safe;
  //   • the auto-un-settle safety net ONLY fires at working/permission;
  //   • the snooze hand-raise ONLY fires at working/permission, and the wake
  //     sweep only at an elapsed wake time;
  //   • auto-settle skips snoozed ids entirely, so a snoozed workspace can't
  //     be settled out from under its own wake timer; the store keeps the two
  //     shelves mutually exclusive from the other side by dropping a snooze
  //     when a settle lands;
  //   • a keep-active pin blocks auto-settle until the agent shows real
  //     activity (a non-null status un-pins via noteActivity's clearPin);
  //     selecting the workspace is not activity and leaves the pin standing,
  //     and a timer wake deliberately touches no pin at all;
  //   • a durable workspace pin blocks every PARK path (manual, bulk, and this
  //     sweep) without mutating a preserved settled/snoozed entry, so unpinning
  //     reveals that base state. Un-park is deliberately not blocked: the
  //     settle safety net and the snooze wake sweep still clear a preserved
  //     entry while the card is pinned, because a pin governs where a card is
  //     shown, not whether its agent went live or its wake time elapsed. Those
  //     two only ever REMOVE a shelf entry, so they cannot fight the pin — the
  //     card is already on top either way, and unpinning then reveals the
  //     up-to-date lifecycle rather than a stale one;
  //   • the "Wrapping up" tier is not a fifth state and mutates nothing — it is
  //     a pure partition of the cards this sweep already left alone, so it can
  //     neither park a row nor be fought over by anything that can. An open PR
  //     is also outside this sweep's PR rule (which wants merged/closed), so
  //     the two never claim the same card.
  // So no two of these effects can act on the same workspace at the same
  // status, and a user-kept or user-deferred card stays put until its agent
  // genuinely runs again.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const store = useSidebarInboxStore.getState();
    const settledSet = new Set(settled.map((e) => e.id));
    const snoozedSet = new Set(snoozed.map((e) => e.id));
    for (const ws of allWorkspaces) {
      const id = ws.workspace_id;
      if (pinOverrides[id] ?? (ws.pinned_at != null)) continue;
      if (settledSet.has(id) || snoozedSet.has(id)) continue;
      if (keepActive[id] === true) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      // Aggregate state: the workspace settles when the *set* is finished,
      // not when its primary happens to merge. Settling a nine-PR stack
      // because its bottom landed would file the card away with eight
      // reviews still open.
      const prs = workspacePrs(ws);
      const prState = prSetSummary(prs).state;
      const stamp = effectiveActivityAt(ws.last_active_at, activity[id]);
      if (
        shouldAutoSettle(
          prState,
          status,
          stamp,
          now,
          // Preserve an explicit shelf drop even for old work, while still
          // applying the merged/closed completion rule above the idle fallback.
          keepActive[id] === "wrapping" ? null : autoSettleDays,
          prsDescribeThisCheckout(prs, ws.git_branch),
        )
      ) {
        store.settle(id, ws.last_active_at ?? undefined);
        emit({ kind: "auto-settled", workspaceId: id });
      }
    }
  }, [
    loaded,
    paneStatuses,
    allWorkspaces,
    settled,
    snoozed,
    keepActive,
    pinOverrides,
    activity,
    autoSettleDays,
    now,
  ]);
}

/** Render-nothing host for `useInboxLifecycle`. Kept as its own component so
 *  the lifecycle's subscriptions (pane statuses, the coarse clock, activity
 *  stamps) re-render only this leaf, never the sidebar view beside it. */
export function InboxLifecycle(): null {
  useInboxLifecycle();
  return null;
}
