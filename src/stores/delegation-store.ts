import { useCallback, useSyncExternalStore } from 'react';
import { delegationList, onDelegationChanged, type LocalTask } from '@/lib/delegation';
import type { ApprovalDecision } from '@/tauri/events';
import type { RemoteApproval } from '@/lib/delegation';
import type { UnlistenFn } from '@tauri-apps/api/event';

export interface DelegationTasksState {
  tasks: readonly LocalTask[];
  loading: boolean;
  error: string | null;
  subscriptionError: string | null;
  subscriptionHealth: 'idle' | 'connecting' | 'connected' | 'failed';
}
const EMPTY: DelegationTasksState = Object.freeze({ tasks: Object.freeze([]), loading: false, error: null, subscriptionError: null, subscriptionHealth: 'idle' });
interface Entry {
  paneId: string;
  threadId: string;
  state: DelegationTasksState;
  listeners: Set<() => void>;
  active: boolean;
  generation: number;
  revision: number;
  unlisten?: UnlistenFn;
  subscription?: Promise<void>;
  inFlight?: Promise<void>;
  dirty: boolean;
  responses: Map<string, RetainedResponse>;
  responsePhases: Map<string, { pending: boolean }>;
}
const entries = new Map<string, Entry>();
const owners = new Map<string, Entry>();
const keyFor = (paneId: string, threadId: string) => JSON.stringify([paneId, threadId]);
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
function frozenTask(task: LocalTask): LocalTask {
  const clone = structuredClone(task);
  const freeze = (value: unknown) => {
    if (value && typeof value === 'object') { Object.values(value).forEach(freeze); Object.freeze(value); }
  };
  freeze(clone);
  return clone;
}
function publish(entry: Entry, patch: Partial<DelegationTasksState>) {
  entry.state = Object.freeze({ ...entry.state, ...patch });
  entry.listeners.forEach(listener => listener());
}
function entryFor(paneId: string, threadId: string): Entry {
  const key = keyFor(paneId, threadId);
  let entry = entries.get(key);
  if (!entry) {
    entry = { paneId, threadId, state: EMPTY, listeners: new Set(), active: false, generation: 0, revision: 0, dirty: false, responses: new Map(), responsePhases: new Map() };
    entries.set(key, entry);
  }
  return entry;
}
function isCurrent(entry: Entry, generation = entry.generation) {
  return entry.active && entry.generation === generation && owners.get(entry.paneId) === entry;
}
function dispose(entry: Entry) {
  entry.responses.clear();
  entry.responsePhases.clear();
  entry.active = false;
  entry.generation++;
  entry.unlisten?.();
  entry.unlisten = undefined;
  if (owners.get(entry.paneId) === entry) owners.delete(entry.paneId);
  entries.delete(keyFor(entry.paneId, entry.threadId));
  publish(entry, EMPTY);
}
async function subscribeNative(entry: Entry): Promise<void> {
  if (entry.subscription) return entry.subscription;
  const generation = entry.generation;
  publish(entry, { subscriptionHealth: 'connecting' });
  entry.subscription = onDelegationChanged(payload => {
    if (isCurrent(entry, generation) && payload.parent_thread_id === entry.threadId) void refreshEntry(entry);
  }).then(unlisten => {
    if (!isCurrent(entry, generation)) { unlisten(); return; }
    entry.unlisten = unlisten;
    publish(entry, { subscriptionHealth: 'connected', subscriptionError: null });
  }).catch(error => {
    if (isCurrent(entry, generation)) publish(entry, { subscriptionHealth: 'failed', subscriptionError: errorText(error) });
  }).finally(() => { entry.subscription = undefined; });
  return entry.subscription;
}
function refreshEntry(entry: Entry): Promise<void> {
  if (!isCurrent(entry)) return Promise.resolve();
  if (entry.inFlight) { entry.dirty = true; return entry.inFlight; }
  const generation = entry.generation;
  publish(entry, { loading: true });
  entry.inFlight = (async () => {
    do {
      entry.dirty = false;
      const revision = entry.revision;
      try {
        const tasks = await delegationList(entry.paneId);
        if (!isCurrent(entry, generation)) return;
        if (entry.revision !== revision) { entry.dirty = true; continue; }
        const unique = new Map(tasks.filter(task => task.parent_thread_id === entry.threadId).map(task => [task.id, frozenTask(task)]));
        publish(entry, { tasks: Object.freeze([...unique.values()]), error: null });
      } catch (error) {
        if (isCurrent(entry, generation)) publish(entry, { error: errorText(error) });
      }
    } while (isCurrent(entry, generation) && entry.dirty);
  })().finally(() => {
    entry.inFlight = undefined;
    if (isCurrent(entry, generation)) publish(entry, { loading: false });
  });
  return entry.inFlight;
}

/** Mount once in the actual pane; additional consumers share its one listener
 * and authoritative list. Invalidations carry no task data and never launch. */
export function useDelegationTasks(paneId: string | null | undefined, threadId: string | null | undefined) {
  const subscribe = useCallback((listener: () => void) => {
    if (!paneId || !threadId) return () => {};
    const entry = entryFor(paneId, threadId);
    entry.listeners.add(listener);
    if (!entry.active) {
      const previous = owners.get(paneId);
      if (previous && previous !== entry) dispose(previous);
      owners.set(paneId, entry);
      entry.active = true;
      void subscribeNative(entry).then(() => { if (isCurrent(entry)) void refreshEntry(entry); });
    }
    return () => {
      entry.listeners.delete(listener);
      if (!entry.listeners.size) dispose(entry);
    };
  }, [paneId, threadId]);
  const snapshot = useCallback(() => paneId && threadId ? entryFor(paneId, threadId).state : EMPTY, [paneId, threadId]);
  const state = useSyncExternalStore(subscribe, snapshot, () => EMPTY);
  const refresh = useCallback(() => refreshDelegationTasks(paneId ?? '', threadId ?? ''), [paneId, threadId]);
  const retrySubscription = useCallback(async () => {
    const entry = paneId ? owners.get(paneId) : undefined;
    if (entry && entry.threadId === threadId && entry.state.subscriptionHealth !== 'connected') await subscribeNative(entry);
    await refresh();
  }, [paneId, threadId, refresh]);
  return { ...state, refresh, retrySubscription };
}
export function isDelegationPaneOwner(paneId: string, threadId: string): boolean {
  const entry = owners.get(paneId);
  return !!entry && entry.threadId === threadId && isCurrent(entry);
}
export function refreshDelegationTasks(paneId: string, threadId: string): Promise<void> {
  const entry = owners.get(paneId);
  return entry?.threadId === threadId ? refreshEntry(entry) : Promise.resolve();
}
/** A read is qualified only in the same live owner and settled phase in which
 * it BEGAN. Phase identity changes on both start and settlement, even on error.
 * These task-wide phases survive detail closure, but not pane-owner disposal. */
export interface ResponseReadBoundary { owner: object; phase: { pending: boolean } | undefined }
export function delegationResponseReadBoundary(paneId: string, threadId: string, taskId: string): ResponseReadBoundary | undefined {
  const entry=owners.get(paneId);
  return entry && isCurrent(entry) && entry.threadId===threadId ? {owner:entry,phase:entry.responsePhases.get(taskId)} : undefined;
}
export function qualifiesDelegationResponseRead(paneId: string, threadId: string, taskId: string, boundary: ResponseReadBoundary | undefined): boolean {
  const current=delegationResponseReadBoundary(paneId,threadId,taskId);
  return !!current && current.owner===boundary?.owner && current.phase===boundary?.phase && !current.phase?.pending;
}
export function beginDelegationResponse(paneId: string, threadId: string, taskId: string): () => void {
  const entry=owners.get(paneId);
  if (!entry || !isCurrent(entry) || entry.threadId!==threadId || entry.responsePhases.get(taskId)?.pending) throw new Error('Approval owner changed or response is still pending.');
  if(!entry.responsePhases.has(taskId) && entry.responsePhases.size>=256) throw new Error('Pane approval phase capacity reached.');
  const phase={pending:true};entry.responsePhases.set(taskId,phase);publish(entry,{});
  return () => {
    if(!isCurrent(entry) || entry.responsePhases.get(taskId)!==phase) return;
    entry.responsePhases.set(taskId,{pending:false});publish(entry,{});
  };
}
export interface RetainedResponse { payloadIdentity: string; decision: ApprovalDecision }
const responseKey = (taskId: string, requestId: string) => JSON.stringify([taskId, requestId]);
/** The actual pane/thread lease owns these bounded, memory-only decisions.
 * Closing a detail does not release them; disposing the pane owner does. */
export function retainedDelegationResponse(paneId: string, threadId: string, taskId: string, requestId: string): RetainedResponse | undefined {
  const entry=owners.get(paneId);
  return entry && isCurrent(entry) && entry.threadId===threadId ? entry.responses.get(responseKey(taskId,requestId)) : undefined;
}
export function retainDelegationResponse(paneId: string, threadId: string, taskId: string, request: RemoteApproval, decision: ApprovalDecision): void {
  const entry=owners.get(paneId);
  if (!entry || !isCurrent(entry) || entry.threadId!==threadId) throw new Error('Approval pane owner changed.');
  const key=responseKey(taskId,request.request_id);
  const payloadIdentity=JSON.stringify([request.request_kind,request.payload]);
  const prior=entry.responses.get(key);
  if (prior) {
    if (prior.payloadIdentity!==payloadIdentity) throw new Error('Approval payload changed; refresh only.');
    return; // The first exact decision stays immutable, including preclaim failure.
  }
  if(entry.responses.size>=256 || payloadIdentity.length>65536 || JSON.stringify(decision).length>65536) throw new Error('Pane approval retention capacity reached.');
  const clone=structuredClone(decision);
  const freeze=(value:unknown)=>{if(value && typeof value==='object'){Object.values(value).forEach(freeze);Object.freeze(value);}};
  freeze(clone);
  entry.responses.set(key,Object.freeze({payloadIdentity,decision:clone}));
}
/** Insert a confirmed launch/action snapshot, never an invented optimistic task. */
export function upsertDelegationTask(paneId: string, threadId: string, task: LocalTask): void {
  const entry = owners.get(paneId);
  if (!entry || !isCurrent(entry) || entry.threadId !== threadId || task.parent_thread_id !== threadId) return;
  const tasks = entry.state.tasks.filter(previous => previous.id !== task.id);
  const index = entry.state.tasks.findIndex(previous => previous.id === task.id);
  tasks.splice(index < 0 ? tasks.length : index, 0, frozenTask(task));
  entry.revision++;
  publish(entry, { tasks: Object.freeze(tasks) });
}
