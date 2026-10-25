/**
 * Compact plugin statuses for the open chat ("Realm live"). Polled only while
 * the chat is on screen and the window is visible; failures and `null` both
 * show nothing, so a broken plugin stays quiet.
 */
import type { PluginInvokeInput, PluginInvokeResult, PluginThreadStatus } from "./contract";
import { pluginStatusHandler, type PluginThreadStatusEntry } from "./model";

export const PLUGIN_THREAD_STATUS_INTERVAL_MS = 30_000;
/** A burst of plugin activity for a thread settles into one status request. */
export const PLUGIN_THREAD_STATUS_ACTIVITY_SETTLE_MS = 1_500;

export interface PluginThreadStatusTarget {
  readonly threadId: string;
  readonly projectId: string | null;
}

export interface PluginThreadStatusValue {
  /** Stable per thread, installation generation and status id. */
  readonly key: string;
  readonly threadId: string;
  readonly entry: PluginThreadStatusEntry;
  readonly status: PluginThreadStatus;
}

export interface PluginThreadStatusPoller {
  /** `entries` must already be limited to active installations granted `ui`. */
  readonly update: (state: {
    readonly target: PluginThreadStatusTarget | null;
    readonly entries: ReadonlyArray<PluginThreadStatusEntry>;
    readonly visible: boolean;
  }) => void;
  /** Ask every status again, e.g. when the window regains focus or a status topic changed. */
  readonly refresh: (installationId?: string) => void;
  readonly subscribe: (listener: () => void) => () => void;
  readonly getSnapshot: () => ReadonlyArray<PluginThreadStatusValue>;
  readonly dispose: () => void;
}

const TONES = new Set(["neutral", "info", "success", "warning", "danger"]);

export function isPluginThreadStatus(value: unknown): value is PluginThreadStatus {
  if (typeof value !== "object" || value === null) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.label === "string" &&
    record.label.length >= 1 &&
    record.label.length <= 40 &&
    typeof record.tone === "string" &&
    TONES.has(record.tone) &&
    (record.detail === undefined || (typeof record.detail === "string" && record.detail.length <= 300))
  );
}

const statusKey = (target: PluginThreadStatusTarget, entry: PluginThreadStatusEntry) =>
  [target.threadId, entry.installation.installationId, entry.installation.generation, entry.item.id].join("\n");

interface Slot {
  readonly key: string;
  readonly target: PluginThreadStatusTarget;
  entry: PluginThreadStatusEntry;
  status: PluginThreadStatus | null;
  /** Asked while a request was in flight; ask once more when it answers. */
  again: boolean;
}

interface PluginThreadActivity {
  readonly threadId: string;
  readonly installationId: string;
}

const activityListeners = new Set<(activity: PluginThreadActivity) => void>();

/** A plugin just ran something for a thread; its status may catch up before the next interval. */
export function notifyPluginThreadActivity(activity: PluginThreadActivity): void {
  for (const listener of activityListeners) listener(activity);
}

const sameStatus = (left: PluginThreadStatus | null, right: PluginThreadStatus | null) =>
  left === right ||
  (left !== null && right !== null && left.label === right.label && left.tone === right.tone && left.detail === right.detail);

const EMPTY: ReadonlyArray<PluginThreadStatusValue> = [];

/**
 * One request in flight per thread, installation generation and status id;
 * an answer for a thread or generation no longer shown is dropped.
 */
export function createPluginThreadStatusPoller(options: {
  readonly invoke: (input: PluginInvokeInput) => Promise<PluginInvokeResult>;
  readonly operationId: () => string;
}): PluginThreadStatusPoller {
  let slots = new Map<string, Slot>();
  const inFlight = new Set<string>();
  let target: PluginThreadStatusTarget | null = null;
  let visible = false;
  let disposed = false;
  let interval: ReturnType<typeof setInterval> | null = null;
  let activityTimer: ReturnType<typeof setTimeout> | null = null;
  let snapshot = EMPTY;
  const listeners = new Set<() => void>();

  const publish = () => {
    const next = [...slots.values()].flatMap((slot): PluginThreadStatusValue[] =>
      slot.status === null
        ? []
        : [{ key: slot.key, threadId: slot.target.threadId, entry: slot.entry, status: slot.status }],
    );
    const unchanged =
      next.length === snapshot.length &&
      next.every((value, index) => {
        const previous = snapshot[index]!;
        return previous.key === value.key && sameStatus(previous.status, value.status);
      });
    if (unchanged) return;
    snapshot = next.length === 0 ? EMPTY : next;
    for (const listener of listeners) listener();
  };

  const ask = (slot: Slot) => {
    if (disposed || !visible) return;
    if (inFlight.has(slot.key)) {
      slot.again = true;
      return;
    }
    inFlight.add(slot.key);
    slot.again = false;
    const { installation, item } = slot.entry;
    const settle = (status: PluginThreadStatus | null) => {
      inFlight.delete(slot.key);
      const current = slots.get(slot.key);
      if (disposed || current === undefined) return;
      if (!sameStatus(current.status, status)) {
        current.status = status;
        publish();
      }
      if (current.again) ask(current);
    };
    options
      .invoke({
        installationId: installation.installationId,
        generation: installation.generation,
        handler: pluginStatusHandler(item.id),
        context: {
          threadId: slot.target.threadId,
          ...(slot.target.projectId === null ? {} : { projectId: slot.target.projectId }),
        },
        operationId: options.operationId(),
      })
      .then(
        (result) => settle(isPluginThreadStatus(result.data) ? result.data : null),
        () => settle(null),
      );
  };

  const askAll = (installationId?: string) => {
    for (const slot of slots.values()) {
      if (installationId === undefined || slot.entry.installation.installationId === installationId) ask(slot);
    }
  };

  const syncInterval = () => {
    const wanted = !disposed && visible && slots.size > 0;
    if (wanted && interval === null) interval = setInterval(() => askAll(), PLUGIN_THREAD_STATUS_INTERVAL_MS);
    else if (!wanted && interval !== null) {
      clearInterval(interval);
      interval = null;
    }
  };

  const pendingActivity = new Set<string>();
  const onActivity = (activity: PluginThreadActivity) => {
    if (target === null || activity.threadId !== target.threadId) return;
    pendingActivity.add(activity.installationId);
    if (activityTimer !== null) return;
    activityTimer = setTimeout(() => {
      activityTimer = null;
      const installationIds = [...pendingActivity];
      pendingActivity.clear();
      for (const installationId of installationIds) askAll(installationId);
    }, PLUGIN_THREAD_STATUS_ACTIVITY_SETTLE_MS);
  };
  activityListeners.add(onActivity);

  return {
    update: (state) => {
      if (disposed) return;
      const becameVisible = state.visible && !visible;
      visible = state.visible;
      if (target?.threadId !== state.target?.threadId) pendingActivity.clear();
      target = state.target;
      const next = new Map<string, Slot>();
      const added: Slot[] = [];
      if (state.target !== null) {
        for (const entry of state.entries) {
          const key = statusKey(state.target, entry);
          const existing = slots.get(key);
          if (existing !== undefined) {
            existing.entry = entry;
            next.set(key, existing);
          } else {
            const slot: Slot = { key, target: state.target, entry, status: null, again: false };
            next.set(key, slot);
            added.push(slot);
          }
        }
      }
      slots = next;
      publish();
      syncInterval();
      if (becameVisible) askAll();
      else for (const slot of added) ask(slot);
    },
    refresh: (installationId) => askAll(installationId),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    getSnapshot: () => snapshot,
    dispose: () => {
      disposed = true;
      activityListeners.delete(onActivity);
      if (activityTimer !== null) clearTimeout(activityTimer);
      activityTimer = null;
      syncInterval();
      slots = new Map();
      listeners.clear();
    },
  };
}
