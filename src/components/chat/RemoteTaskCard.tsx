import { useEffect, useRef, useState } from 'react';
import { AlertCircle, CheckCircle2, Circle, LoaderCircle, Server, Square } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { delegationCancel, delegationRead, type LocalTask, type TaskStatus } from '@/lib/delegation';
import { isDelegationPaneOwner, refreshDelegationTasks, upsertDelegationTask } from '@/stores/delegation-store';

export interface RemoteTaskCardProps {
  task: LocalTask;
  paneId: string;
  writable: boolean;
  onOpen: (task: LocalTask) => void;
}
export const remoteTaskStatusLabel: Record<TaskStatus, string> = {
  starting: 'Starting', running: 'Running', awaiting_approval: 'Approval needed', stopping: 'Stopping', completed: 'Completed', failed: 'Failed', cancelled: 'Stopped', interrupted: 'Interrupted',
};
export const isRemoteTaskActive = (task: LocalTask) => ['starting', 'running', 'awaiting_approval', 'stopping'].includes(task.status);
export function deliveryExplanation(task: LocalTask): string {
  if (task.delivery?.outcome==='accepted') return 'Parent accepted this result. Resending is forbidden.';
  if (task.delivery?.outcome==='unknown') return 'Parent delivery outcome is unknown. Resending is forbidden.';
  if (task.delivery?.eligible) return 'This recorded result is unsent. Open task to deliver to parent; this does not rerun the remote task.';
  const reasons = {cancelled:'Task was cancelled; parent delivery is unavailable.',no_result:'No remote result is recorded.',not_terminal:'Remote execution has not settled.',in_flight:'Parent delivery is in progress.',already_delivered:'Delivered to parent.',tail_pending:'Recorded history is still being reconciled.',authority_unavailable:'Current parent or grant authority is unavailable.',accepted:'Parent accepted this result. Resending is forbidden.',unknown:'Parent delivery outcome is unknown. Resending is forbidden.'};
  return task.delivery?.reason ? reasons[task.delivery.reason] : 'Parent delivery is unavailable. Refresh the saved task state.';
}
export function RemoteTaskStatus({ task }: { task: LocalTask }) {
  const Icon = task.status === 'completed' ? CheckCircle2 : task.status === 'cancelled' ? Square : ['failed', 'interrupted', 'awaiting_approval'].includes(task.status) ? AlertCircle : isRemoteTaskActive(task) ? LoaderCircle : Circle;
  return <span className="inline-flex items-center gap-1.5 text-body-sm font-medium"><Icon aria-hidden className={`size-3.5 ${['starting', 'running', 'stopping'].includes(task.status) ? 'motion-safe:animate-spin' : ''}`} />{remoteTaskStatusLabel[task.status]}</span>;
}
export function RemoteTaskCard({ task, paneId, writable, onOpen }: RemoteTaskCardProps) {
  const key = JSON.stringify([paneId, task.parent_thread_id, task.id]);
  const latest = useRef({ key, writable, task });
  latest.current = { key, writable, task };
  const mounted = useRef(false);
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ key: string; message: string } | null>(null);
  const [requested, setRequested] = useState<string | null>(null);
  useEffect(() => {
    mounted.current = true;
    pending.current = false;
    setBusy(false);
    return () => { mounted.current = false; };
  }, [key]);
  const canAct = writable && isDelegationPaneOwner(paneId, task.parent_thread_id);
  const active = isRemoteTaskActive(task);
  const stopPending = task.cancel_requested || task.status === 'stopping' || requested === key;
  const stop = async () => {
    if (pending.current || !latest.current.writable || latest.current.key !== key || !isDelegationPaneOwner(paneId, task.parent_thread_id) || !isRemoteTaskActive(latest.current.task)) return;
    pending.current = true;
    setBusy(true);
    setError(null);
    const current = () => mounted.current && latest.current.key === key && latest.current.writable && isDelegationPaneOwner(paneId, task.parent_thread_id);
    try {
      await delegationCancel(paneId, task.id);
      if (!current()) return;
      setRequested(key);
      const readback = await delegationRead(paneId, task.id, 0);
      if (!current()) return;
      if (readback.task.id !== task.id || readback.task.parent_thread_id !== task.parent_thread_id) throw new Error('Task readback did not match this conversation.');
      upsertDelegationTask(paneId, task.parent_thread_id, readback.task);
    } catch (cause) {
      if (current()) setError({ key, message: `Stop unconfirmed: ${cause instanceof Error ? cause.message : String(cause)}. Retry stop to confirm; the remote task may still be running.` });
    } finally {
      if (mounted.current && latest.current.key === key) { pending.current = false; setBusy(false); }
    }
  };
  const message = error?.key === key ? error.message : null;
  const requests = task.remote?.pending_requests ?? [];
  const waiting = task.status === 'awaiting_approval' && task.remote?.status === 'awaiting_approval' && !busy && !stopPending && !task.remote.cancel_requested && requests.length > 0;
  const questionPending = requests.some(request => request.request_kind === 'user-input');
  const approvalPending = requests.some(request => request.request_kind !== 'user-input');
  const waitingSummary = waiting ? questionPending ? approvalPending ? 'Waiting for question and approval responses' : 'Waiting for question response' : 'Waiting for approval response' : null;
  const summary = task.remote?.error ?? task.remote?.result ?? waitingSummary ?? task.remote?.activity;
  return <article className="my-2 grid gap-2 rounded-lg border border-border bg-surface-1 px-3 py-2.5" aria-label={`Remote task: ${task.title || 'Delegated task'}`}>
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <Server aria-hidden className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1"><p className="truncate text-body font-medium">{task.title || 'Delegated task'}</p><p className="truncate text-body-sm text-muted-foreground" title={task.target_ssh_target}>{task.target_host_name}</p></div>
      <RemoteTaskStatus task={task} />
      <div className="flex items-center gap-1">
        <Button type="button" size="sm" variant="ghost" onClick={() => onOpen(task)}>View task</Button>
        {active && <Button type="button" size="sm" variant="outline" aria-label={message ? 'Retry stop' : 'Stop task'} disabled={!canAct || busy || (stopPending && !message && !task.connection_error)} onClick={() => void stop()}>{busy ? 'Requesting stop…' : message ? 'Retry stop' : stopPending ? 'Stopping…' : 'Stop'}</Button>}
      </div>
    </div>
    <p className="text-caption text-muted-foreground">App receipt{task.parent_event_id != null ? ` · Parent event #${task.parent_event_id}` : ''} · Receipts within work are shown after the group.</p>
    {summary && <p className={`truncate text-body-sm ${task.remote?.error ? 'text-destructive' : 'text-muted-foreground'}`} title={summary}>{summary}</p>}
    {task.connection_error && <div className="flex flex-wrap items-center justify-between gap-2"><p className="text-body-sm text-muted-foreground">Reconnecting · Last known: {remoteTaskStatusLabel[task.status]}. {task.connection_error}</p><Button type="button" size="sm" variant="ghost" onClick={() => { if (isDelegationPaneOwner(paneId, task.parent_thread_id)) void refreshDelegationTasks(paneId, task.parent_thread_id); }}>Refresh status</Button></div>}
    {stopPending && active && <p role="status" className="text-body-sm text-muted-foreground">Stop requested; awaiting remote confirmation.</p>}
    {['held','suppressed'].includes(task.wake_state) && <p className="text-body-sm text-muted-foreground">{deliveryExplanation(task)}</p>}
    {task.wake_error && <p className={`text-body-sm ${canAct && !message && !task.connection_error && !task.remote?.error && task.status === 'cancelled' && task.cancel_requested && task.wake_state === 'suppressed' && task.delivery?.outcome === 'not_attempted' && task.delivery.eligible === false && task.delivery.reason === 'cancelled' ? 'text-muted-foreground' : 'text-destructive'}`}>Parent delivery: {task.wake_error}</p>}
    {message && <p role="alert" className="text-body-sm text-destructive">{message}</p>}
  </article>;
}
