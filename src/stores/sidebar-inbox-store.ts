import { create } from "zustand";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";

/** Persisted UI-state key holding the settled-inbox JSON blob. "Settled" is a
 *  purely visual inbox flag — the workspace is untouched (no archive, no
 *  close); its card just collapses to a one-line row under the "Settled"
 *  divider until the user un-settles it.
 *
 *  The key name predates snooze; the blob now carries the snooze shelf too.
 *  Renaming it would strand every existing install's settled list, so the
 *  historical name stays.
 *
 *  The persisted value is an object (see {@link PersistedInbox}). Old installs
 *  wrote a bare JSON array of settled entries; that shape is still read on
 *  load for back-compat. */
export const SETTLED_UI_STATE_KEY = "sidebar.inbox.settled";

export interface SettledEntry {
  id: string;
  /** When the card was settled (ms epoch) — the sweep timestamp. */
  at: number;
  /** When the WORK ended, if known — the shelf orders and labels by this so
   *  history reads chronologically rather than by when the sweep happened.
   *  Falls back to `at` when unknown. */
  workEndedAt?: number;
}

export interface SnoozeEntry {
  id: string;
  /** When the snooze was set (ms epoch). Nothing branches on it: the wake
   *  paths key off `until` or off a live status change, and no "is this
   *  hand-raise newer than the snooze?" check is needed, because a workspace
   *  cannot be snoozed while it is working or awaiting permission — so any
   *  hand-raise a snoozed workspace makes is necessarily a transition that
   *  happened after this timestamp. It is kept because it is the only record
   *  of *when* the deferral was made (the shelf shows time-until-wake, not
   *  how long ago the user deferred) and because the parser validates an
   *  entry's shape by it. */
  at: number;
  /** When the workspace should return to the active list (ms epoch). */
  until: number;
}

/** The one timestamp a settled row is ordered and labelled by. Sort key and
 *  display label both go through here so a row can never sit in one place in
 *  the list while claiming a different time — a manually swept-aside old
 *  workspace would otherwise jump to the top of history. */
export function resolveSettledTimestamp(entry: SettledEntry): number {
  return entry.workEndedAt ?? entry.at;
}

/** The full persisted inbox blob. */
interface PersistedInbox {
  settled: SettledEntry[];
  /** Workspaces the user asked to see again later, with their wake times.
   *  Persisted so a snooze survives a restart — a snooze that evaporated on
   *  relaunch would silently drop work the user deliberately deferred. */
  snoozed: SnoozeEntry[];
  /** Workspace ids the user explicitly un-settled — a "keep active" pin that
   *  suppresses auto-settle until real agent activity clears it. */
  keepActive: string[];
  /** Last-observed activity (ms epoch) per workspace. Client-tracked because
   *  the backend stamps no status-changed-at. */
  activity: Record<string, number>;
}

/** How long an activity stamp must jump before we bother re-persisting. The
 *  activity observer fires from render effects every coarse tick, so without a
 *  throttle it would write on every clock tick for a long-running agent. */
const ACTIVITY_WRITE_THROTTLE_MS = 60_000;

interface SidebarInboxStore {
  /** True once the persisted blob has been read (or failed to read — either
   *  way the inbox can render without flashing settled rows late). */
  loaded: boolean;
  /** Workspaces the user has settled, newest-settled first (matches the
   *  visual order of the settled section). Persisted. */
  settled: SettledEntry[];
  /** Workspaces deferred to a wake time — the shelf that sits above Settled.
   *  Mutually exclusive with `settled`: a workspace on both shelves would
   *  render twice and give the user two contradictory affordances for the
   *  same card. Persisted. */
  snoozed: SnoozeEntry[];
  /** "Keep active" pins keyed by workspace id — set when the user un-settles a
   *  card, cleared when its agent next shows activity or the user settles it
   *  again. While pinned, auto-settle skips the workspace. Persisted. */
  keepActive: Record<string, true>;
  /** Last-observed activity ms-epoch per workspace, used by the inactivity
   *  auto-settle rule. Persisted. */
  activity: Record<string, number>;
  /** Active repo filter — a project path, or null for "All". Session-only:
   *  a fresh launch always starts on All. */
  filter: string | null;
  /** Load the persisted blob once at inbox mount. Idempotent. */
  load: () => Promise<void>;
  /** Settle a card. Also clears any keep-active pin on it (an explicit settle
   *  ends the pin) and drops it off the snooze shelf, which the settled shelf
   *  supersedes. Used by both the manual gesture and background auto-settle.
   *
   *  `workEndedAt` is when the work itself finished, when the caller knows it;
   *  without it the row falls back to its sweep time and stale work sorts to
   *  the top of history. */
  settle: (workspaceId: string, workEndedAt?: number) => void;
  /** Un-settle a card. `reason` is required: "user" (the settled-row button)
   *  sets a keep-active pin so auto-settle leaves it alone until its agent
   *  runs again; "activity" (a live agent resurfaced it) clears any pin. */
  unsettle: (workspaceId: string, reason: "user" | "activity") => void;
  /** Snooze a workspace until `until` (ms epoch) — "come back to me later".
   *  Replaces any existing snooze on the same id so re-snoozing just moves the
   *  wake time. Clears the workspace's settled entry (the two shelves are
   *  mutually exclusive) and its keep-active pin, since deferring a card is
   *  the opposite of asking to keep it in the active list. */
  snooze: (workspaceId: string, until: number) => void;
  /** Wake a snoozed workspace. `reason` decides what happens to the
   *  keep-active pin, mirroring {@link SidebarInboxStore.unsettle}:
   *  "user" (an explicit "Wake now") pins it so auto-settle leaves it alone
   *  until its agent runs again; "activity" (the agent resurfaced it on its
   *  own) clears any pin, because the agent running is exactly the signal a
   *  pin waits for; "timer" (the wake time simply arrived) touches no pin at
   *  all — the clock expiring says nothing about what the user wants next. */
  unsnooze: (workspaceId: string, reason: "user" | "activity" | "timer") => void;
  /** Record that a workspace showed activity at `at` (ms epoch). The stamp is
   *  throttled: it only moves when `at` exceeds the stored stamp by more than
   *  a minute, so it stays cheap to call from per-tick render effects.
   *
   *  The keep-active pin is cleared ONLY with `opts.clearPin` — reserved for
   *  genuine agent activity (a non-null status). Merely selecting a workspace,
   *  or laying down a first-seen baseline, just stamps and leaves the pin
   *  intact, so a card the user kept active stays active. The pin still clears
   *  when `clearPin` is set even if the stamp itself is throttled. */
  noteActivity: (
    workspaceId: string,
    at: number,
    opts?: { clearPin?: boolean },
  ) => void;
  setFilter: (projectPath: string | null) => void;
  /** Drop settled and snoozed entries — plus keep-active pins and activity
   *  stamps — whose workspace no longer exists (archived / deleted / closed) so
   *  the persisted blob can't grow without bound. Only prunes — and only
   *  persists — when something actually vanished. */
  prune: (validIds: Set<string>) => void;
}

let loadPromise: Promise<void> | null = null;

function persist(state: {
  settled: SettledEntry[];
  snoozed: SnoozeEntry[];
  keepActive: Record<string, true>;
  activity: Record<string, number>;
}): void {
  const blob: PersistedInbox = {
    settled: state.settled,
    snoozed: state.snoozed,
    keepActive: Object.keys(state.keepActive),
    activity: state.activity,
  };
  dbSetUiState(SETTLED_UI_STATE_KEY, JSON.stringify(blob)).catch(console.error);
}

interface ParsedInbox {
  settled: SettledEntry[];
  snoozed: SnoozeEntry[];
  keepActive: Record<string, true>;
  activity: Record<string, number>;
}

function parseSettledEntries(value: unknown): SettledEntry[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (x): x is SettledEntry =>
      typeof x === "object" &&
      x !== null &&
      typeof (x as SettledEntry).id === "string" &&
      typeof (x as SettledEntry).at === "number",
  );
}

function parseSnoozeEntries(value: unknown): SnoozeEntry[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (x): x is SnoozeEntry =>
      typeof x === "object" &&
      x !== null &&
      typeof (x as SnoozeEntry).id === "string" &&
      typeof (x as SnoozeEntry).at === "number" &&
      typeof (x as SnoozeEntry).until === "number",
  );
}

function parseInbox(raw: string | null): ParsedInbox {
  const empty: ParsedInbox = { settled: [], snoozed: [], keepActive: {}, activity: {} };
  if (!raw) return empty;
  try {
    const parsed: unknown = JSON.parse(raw);
    // Back-compat: old installs persisted a bare array of settled entries.
    // Blobs written before snooze existed simply carry no `snoozed` key, so
    // both older shapes land on an empty snooze shelf rather than failing.
    if (Array.isArray(parsed)) {
      return {
        settled: parseSettledEntries(parsed),
        snoozed: [],
        keepActive: {},
        activity: {},
      };
    }
    if (typeof parsed !== "object" || parsed === null) return empty;
    const obj = parsed as Record<string, unknown>;
    const keepActive: Record<string, true> = {};
    if (Array.isArray(obj.keepActive)) {
      for (const id of obj.keepActive) {
        if (typeof id === "string") keepActive[id] = true;
      }
    }
    const activity: Record<string, number> = {};
    if (typeof obj.activity === "object" && obj.activity !== null) {
      for (const [id, at] of Object.entries(obj.activity as Record<string, unknown>)) {
        if (typeof at === "number") activity[id] = at;
      }
    }
    return {
      settled: parseSettledEntries(obj.settled),
      snoozed: parseSnoozeEntries(obj.snoozed),
      keepActive,
      activity,
    };
  } catch {
    // Corrupt value — start clean rather than crash the sidebar.
    return empty;
  }
}

export const useSidebarInboxStore = create<SidebarInboxStore>((set, get) => ({
  loaded: false,
  settled: [],
  snoozed: [],
  keepActive: {},
  activity: {},
  filter: null,

  load: () => {
    if (loadPromise) return loadPromise;
    loadPromise = dbGetUiState(SETTLED_UI_STATE_KEY)
      .then((raw) => {
        const parsed = parseInbox(raw);
        set({
          settled: parsed.settled,
          snoozed: parsed.snoozed,
          keepActive: parsed.keepActive,
          activity: parsed.activity,
          loaded: true,
        });
      })
      .catch(() => {
        set({ loaded: true });
      });
    return loadPromise;
  },

  settle: (workspaceId, workEndedAt) => {
    const { settled, snoozed, keepActive, activity } = get();
    const alreadySettled = settled.some((e) => e.id === workspaceId);
    const pinned = keepActive[workspaceId] === true;
    const wasSnoozed = snoozed.some((e) => e.id === workspaceId);
    if (alreadySettled && !pinned && !wasSnoozed) return;
    const entry: SettledEntry = { id: workspaceId, at: Date.now() };
    // Only carry the key when the caller actually knows the work's end time —
    // an explicit `undefined` would serialize as a key the parser then has to
    // treat as "unknown" anyway.
    if (workEndedAt !== undefined) entry.workEndedAt = workEndedAt;
    const nextSettled = alreadySettled ? settled : [entry, ...settled];
    const nextSnoozed = wasSnoozed
      ? snoozed.filter((e) => e.id !== workspaceId)
      : snoozed;
    let nextKeepActive = keepActive;
    if (pinned) {
      nextKeepActive = { ...keepActive };
      delete nextKeepActive[workspaceId];
    }
    set({ settled: nextSettled, snoozed: nextSnoozed, keepActive: nextKeepActive });
    persist({
      settled: nextSettled,
      snoozed: nextSnoozed,
      keepActive: nextKeepActive,
      activity,
    });
  },

  unsettle: (workspaceId, reason) => {
    const { settled, snoozed, keepActive, activity } = get();
    const wasSettled = settled.some((e) => e.id === workspaceId);
    const pinned = keepActive[workspaceId] === true;
    // "user" adds a pin; "activity" clears one. Nothing to do if neither the
    // settled entry nor the pin state would change.
    const willPin = reason === "user";
    const pinChanges = willPin ? !pinned : pinned;
    if (!wasSettled && !pinChanges) return;
    const nextSettled = wasSettled
      ? settled.filter((e) => e.id !== workspaceId)
      : settled;
    let nextKeepActive = keepActive;
    if (willPin && !pinned) {
      nextKeepActive = { ...keepActive, [workspaceId]: true };
    } else if (!willPin && pinned) {
      nextKeepActive = { ...keepActive };
      delete nextKeepActive[workspaceId];
    }
    set({ settled: nextSettled, keepActive: nextKeepActive });
    persist({ settled: nextSettled, snoozed, keepActive: nextKeepActive, activity });
  },

  snooze: (workspaceId, until) => {
    const { settled, snoozed, keepActive, activity } = get();
    // Re-snoozing replaces the entry outright rather than editing `until` in
    // place, so `at` describes the deferral that is actually in force instead
    // of the first one the user ever made on this workspace.
    const nextSnoozed: SnoozeEntry[] = [
      { id: workspaceId, at: Date.now(), until },
      ...snoozed.filter((e) => e.id !== workspaceId),
    ];
    const nextSettled = settled.filter((e) => e.id !== workspaceId);
    let nextKeepActive = keepActive;
    if (keepActive[workspaceId] === true) {
      nextKeepActive = { ...keepActive };
      delete nextKeepActive[workspaceId];
    }
    set({ settled: nextSettled, snoozed: nextSnoozed, keepActive: nextKeepActive });
    persist({
      settled: nextSettled,
      snoozed: nextSnoozed,
      keepActive: nextKeepActive,
      activity,
    });
  },

  unsnooze: (workspaceId, reason) => {
    const { settled, snoozed, keepActive, activity } = get();
    const wasSnoozed = snoozed.some((e) => e.id === workspaceId);
    const pinned = keepActive[workspaceId] === true;
    // Same pin rules as unsettle, plus "timer": a wake time simply arriving is
    // not a user decision, so it must not create a pin that would then suppress
    // auto-settle on work nobody has looked at yet.
    const willPin = reason === "user";
    const willUnpin = reason === "activity" && pinned;
    const pinChanges = willPin ? !pinned : willUnpin;
    if (!wasSnoozed && !pinChanges) return;
    const nextSnoozed = wasSnoozed
      ? snoozed.filter((e) => e.id !== workspaceId)
      : snoozed;
    let nextKeepActive = keepActive;
    if (willPin && !pinned) {
      nextKeepActive = { ...keepActive, [workspaceId]: true };
    } else if (willUnpin) {
      nextKeepActive = { ...keepActive };
      delete nextKeepActive[workspaceId];
    }
    set({ snoozed: nextSnoozed, keepActive: nextKeepActive });
    persist({ settled, snoozed: nextSnoozed, keepActive: nextKeepActive, activity });
  },

  noteActivity: (workspaceId, at, opts) => {
    const { settled, snoozed, keepActive, activity } = get();
    const prev = activity[workspaceId];
    // Only genuine agent activity un-pins a kept-active card; selection and
    // first-seen baselines merely stamp.
    const clearPin = opts?.clearPin === true;
    const pinned = clearPin && keepActive[workspaceId] === true;
    // Write-throttle: only persist when the stamp jumps by more than a minute
    // (or on first observation). A pin still clears if it's set even when the
    // stamp itself is throttled, so a resurfacing agent always un-pins.
    const stampChanges = prev === undefined || at - prev > ACTIVITY_WRITE_THROTTLE_MS;
    if (!stampChanges && !pinned) return;
    const nextActivity = stampChanges ? { ...activity, [workspaceId]: at } : activity;
    let nextKeepActive = keepActive;
    if (pinned) {
      nextKeepActive = { ...keepActive };
      delete nextKeepActive[workspaceId];
    }
    set({ activity: nextActivity, keepActive: nextKeepActive });
    persist({ settled, snoozed, keepActive: nextKeepActive, activity: nextActivity });
  },

  setFilter: (projectPath) => set({ filter: projectPath }),

  prune: (validIds) => {
    const { settled, snoozed, keepActive, activity } = get();
    const nextSettled = settled.filter((e) => validIds.has(e.id));
    const nextSnoozed = snoozed.filter((e) => validIds.has(e.id));
    const nextKeepActive: Record<string, true> = {};
    for (const id of Object.keys(keepActive)) {
      if (validIds.has(id)) nextKeepActive[id] = true;
    }
    const nextActivity: Record<string, number> = {};
    for (const [id, at] of Object.entries(activity)) {
      if (validIds.has(id)) nextActivity[id] = at;
    }
    const changed =
      nextSettled.length !== settled.length ||
      nextSnoozed.length !== snoozed.length ||
      Object.keys(nextKeepActive).length !== Object.keys(keepActive).length ||
      Object.keys(nextActivity).length !== Object.keys(activity).length;
    if (!changed) return;
    set({
      settled: nextSettled,
      snoozed: nextSnoozed,
      keepActive: nextKeepActive,
      activity: nextActivity,
    });
    persist({
      settled: nextSettled,
      snoozed: nextSnoozed,
      keepActive: nextKeepActive,
      activity: nextActivity,
    });
  },
}));

/** Test-only: reset the module-level load memoization + store state. */
export function __resetSidebarInboxStoreForTests(): void {
  loadPromise = null;
  useSidebarInboxStore.setState({
    loaded: false,
    settled: [],
    snoozed: [],
    keepActive: {},
    activity: {},
    filter: null,
  });
}
