import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, ChevronRight, Server } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { delegationCancel, delegationDeliver, delegationRead, delegationRespond, type LocalTask, type RemoteApproval, type TaskEvent } from '@/lib/delegation';
import type { ApprovalDecision } from '@/tauri/events';
import { beginDelegationResponse, delegationResponseReadBoundary, qualifiesDelegationResponseRead, type ResponseReadBoundary, isDelegationPaneOwner, retainedDelegationResponse, retainDelegationResponse, upsertDelegationTask } from '@/stores/delegation-store';
import { deliveryExplanation, isRemoteTaskActive, RemoteTaskStatus } from './RemoteTaskCard';
import { extractQuestions } from './ComposerPendingInputPanel';
import { QuestionForm } from './QuestionForm';

function RemoteQuestion({request,disabled,retained,isCurrent,onSubmit}:{request:RemoteApproval;disabled:boolean;retained:boolean;isCurrent:()=>boolean;onSubmit:(decision:ApprovalDecision)=>Promise<void>}) {
  const questions=extractQuestions(request.payload);
  const [submitted,setSubmitted]=useState(false);
  const payload=request.payload && typeof request.payload==='object' && !Array.isArray(request.payload) ? request.payload as Record<string,unknown> : null;
  if (!payload || !Array.isArray(payload.questions) || !questions.length || questions.length!==payload.questions.length || new Set(questions.map(q=>q.question)).size!==questions.length) {
    return <p role="status">Unsupported question format. No unanswered Allow will be sent.</p>;
  }
  return <fieldset disabled={disabled || submitted || retained} className="min-w-0">
    <legend className="text-body-sm font-medium">Answer remote questions</legend>
    <QuestionForm questions={questions} idPrefix={`remote-${request.request_id}`} globalShortcuts={false} onSubmit={async answers=>{
      if(!isCurrent()) throw new Error('Displayed approval owner is no longer current.');
      if(disabled || submitted || retained || answers.length!==questions.length || answers.some(answer=>!answer.trim())) return;
      setSubmitted(true);
      await onSubmit({decision:'allow',updated_input:{...payload,answers:Object.fromEntries(questions.map((q,index)=>[q.question,answers[index]]))}});
    }} />
    {submitted && <p role="status">Answer submitted; refresh the saved task to confirm. Answers cannot be changed or blindly resent.</p>}
  </fieldset>;
}

function approvalSummary(request: RemoteApproval) {
  const record = (value: unknown): Record<string, unknown> | null => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
  const text = (value: unknown) => typeof value === 'string' && value.trim() ? value : null;
  const payload = record(request.payload);
  // Claude forwards the original tool input alone. No key in that raw object
  // (including input/tool_input/tool_name) is trusted presentation metadata.
  if (request.request_kind === 'other') {
    return { command: null, reason: null, cwd: null, path: null, scope: null, title: payload ? 'Use remote tool' : null, supported: !!payload, generic: true };
  }
  const inputValue = payload && ('tool_input' in payload ? payload.tool_input : payload.input);
  const input = record(inputValue);
  const validInputs = !!payload && ['tool_input', 'input'].every(field => !(field in payload) || !!record(payload[field]))
    && (!('tool_input' in payload) || !('input' in payload) || JSON.stringify(payload.tool_input) === JSON.stringify(payload.input));
  const coherentText = (field: string, nullable = false) => {
    const values = [payload, input].filter((value): value is Record<string, unknown> => !!value && field in value).map(value => value[field]);
    return values.every(value => nullable && value === null || !!text(value))
      && new Set(values.filter(value => value !== null)).size <= 1;
  };
  const directCommand = text(payload?.command);
  const toolCommand = text(input?.command);
  const command = directCommand ?? toolCommand;
  // Presentation must not "repair" malformed or contradictory consent by
  // silently preferring an alternate input/command field.
  const valid = !!payload && validInputs
    && (!('command' in payload) || !!directCommand)
    && (!input || !('command' in input) || !!toolCommand)
    && (!directCommand || !toolCommand || directCommand === toolCommand)
    && coherentText('file_path') && coherentText('cwd', true) && coherentText('grantRoot', true);
  const reason = text(payload?.reason) ?? text(payload?.description) ?? text(input?.reason) ?? text(input?.description);
  const cwd = text(payload?.cwd) ?? text(input?.cwd);
  const path = text(payload?.file_path) ?? text(input?.file_path);
  const scope = text(payload?.grantRoot) ?? text(input?.grantRoot);
  const title = !valid ? null : ['command', 'permission'].includes(request.request_kind) && command ? 'Run command'
    : request.request_kind === 'file-change' && (path || scope) ? 'Change remote files'
    : request.request_kind === 'file-read' && path ? 'Read remote files' : null;
  return { command, reason, cwd, path, scope, title, supported: title !== null, generic: false };
}
function RemoteApprovalDetails({ request, task }: { request: RemoteApproval; task: LocalTask }) {
  const { command, reason, cwd, path, scope, title, supported, generic } = approvalSummary(request);
  return <div className="grid gap-2">
    <p className="text-body-sm font-medium">{title ?? 'Approval requested'} on {task.target_host_name}</p>
    {command && <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono text-body-sm">{command}</pre>}
    {reason && <p className="whitespace-pre-wrap break-words text-body-sm">{reason}</p>}
    <p className="break-all text-body-sm text-muted-foreground">{cwd ? 'Working directory' : 'Checkout'} on {task.target_host_name}: {cwd ?? task.target_workspace_path}</p>
    {path && <p className="break-all text-body-sm text-muted-foreground">Requested path on {task.target_host_name}: {path}</p>}
    {scope && <p className="break-all text-body-sm text-muted-foreground">Requested scope on {task.target_host_name}: {scope}</p>}
    {generic && supported && <><p className="text-body-sm text-muted-foreground">Tool identity is not included in this approval. Review the complete original input before allowing this one request.</p><pre aria-label="Original tool input" className="max-h-64 overflow-auto whitespace-pre-wrap break-all font-mono text-body-sm">{JSON.stringify(request.payload, null, 2)}</pre></>}
    {!supported && <p role="status" className="text-body-sm text-muted-foreground">Unsupported approval format. Allow is unavailable; you can deny this request.</p>}
    <details><summary className="cursor-pointer text-body-sm text-muted-foreground">Full request details</summary><pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all font-mono text-body-sm">{JSON.stringify(request, null, 2)}</pre></details>
  </div>;
}

export interface RemoteTaskDetailProps { task: LocalTask; paneId: string; writable: boolean; onBack: () => void }
interface History { key: string; revision: string; responseBoundary: ResponseReadBoundary | undefined; writable: boolean; task: LocalTask; events: TaskEvent[]; cursor: number; hasMore: boolean }
interface StopConfirmation { settled: boolean }
function eventText({ event }: TaskEvent): string | null {
  switch (event.type) {
    case 'item_completed':
      if (event.item.kind === 'assistant_text' || event.item.kind === 'assistant_thinking') return event.item.text;
      if (event.item.kind === 'tool_use') return `${event.item.tool_name}\n${JSON.stringify(event.item.input, null, 2)}`;
      if (event.item.kind === 'tool_result') return typeof event.item.content === 'string' ? event.item.content : JSON.stringify(event.item.content, null, 2);
      return null;
    case 'content_delta': return event.delta.kind === 'tool_input' ? event.delta.partial_json : event.delta.text;
    case 'runtime_warning': return event.message;
    case 'request_response_failed': return event.message;
    default: return null;
  }
}
export function RemoteTaskDetail(props: RemoteTaskDetailProps) {
  const {task,paneId}=props;
  const owner=delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner;
  const [lifecycle,setLifecycle]=useState({owner,alternate:false});
  // Replace local presentation at the CURRENT lease boundary, never from an
  // obsolete promise. Two alternating keys suffice; no global owner IDs exist.
  if(lifecycle.owner!==owner) { setLifecycle({owner,alternate:!lifecycle.alternate});return null; }
  return <OwnedRemoteTaskDetail key={JSON.stringify([paneId,task.parent_thread_id,task.id,lifecycle.alternate])} {...props}/>;
}
function OwnedRemoteTaskDetail({ task, paneId, writable, onBack }: RemoteTaskDetailProps) {
  const key = JSON.stringify([paneId, task.parent_thread_id, task.id]);
  const revision = JSON.stringify([task.updated_at, task.status, task.cancel_requested, task.connection_error, task.wake_state, task.delivery, task.responses]);
  const latest = useRef({ key, revision, writable, task, onBack });
  latest.current = { key, revision, writable, task, onBack };
  const generation = useRef(0);
  const mounted = useRef(false);
  const readPending = useRef(false);
  const historyRef = useRef<History | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [loading, setLoading] = useState(false);
  const [readError, setReadError] = useState<{ key: string; message: string } | null>(null);
  const actionPending = useRef(false);
  const [action, setAction] = useState<string | null>(null);
  const [actionError, setActionError] = useState<{ key: string; message: string; requestId?: string; stop?: StopConfirmation } | null>(null);
  const stopConfirmation = useRef<StopConfirmation | null>(null);
  const [stopRequested, setStopRequested] = useState<string | null>(null);
  const read = async (force = false, confirm = false): Promise<LocalTask | undefined> => {
    if (readPending.current && !force) return;
    readPending.current = true;
    const token = ++generation.current;
    const responseBoundary=delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id);
    const beganWritable=writable;
    // Capture at BEGIN: a read issued during Cancel cannot become a qualified
    // confirmation merely because that same mutation settles before it returns.
    const beganStop=stopConfirmation.current?.settled ? stopConfirmation.current : null;
    const previous = historyRef.current?.key === key ? historyRef.current : null;
    setLoading(true);
    setReadError(null);
    try {
      const response = await delegationRead(paneId, task.id, previous?.cursor ?? 0);
      if (token !== generation.current || latest.current.key !== key || responseBoundary?.owner!==delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner) return;
      if (response.task.id !== task.id || response.task.parent_thread_id !== task.parent_thread_id) throw new Error('Task history did not match this parent conversation.');
      if (response.has_more && response.next_cursor <= (previous?.cursor ?? 0)) throw new Error('The receiver did not advance its event cursor. Refresh the host helper before retrying.');
      const events = [...new Map([...(previous?.events ?? []), ...response.events].map(event => [event.sequence, event])).values()].sort((a, b) => a.sequence - b.sequence);
      const next = { key, revision, responseBoundary, writable:beganWritable, task: response.task, events, cursor: response.next_cursor, hasMore: response.has_more };
      historyRef.current = next;
      setHistory(next);
      setActionError(previous=>{
        if(previous?.key===key && previous.stop && previous.stop===beganStop && beganStop===stopConfirmation.current && beganWritable && latest.current.writable && qualifiesDelegationResponseRead(paneId,task.parent_thread_id,task.id,responseBoundary) && response.task.cancel_requested) return null;
        if(previous?.key!==key || !previous.requestId || !beganWritable || !latest.current.writable || !qualifiesDelegationResponseRead(paneId,task.parent_thread_id,task.id,responseBoundary)) return previous;
        const request=response.task.remote?.pending_requests.find(request=>request.request_id===previous.requestId);
        const retained=retainedDelegationResponse(paneId,task.parent_thread_id,task.id,previous.requestId);
        const projected=response.task.responses?.find(projection=>projection.request_id===previous.requestId);
        return projected && request && retained?.payloadIdentity===JSON.stringify([request.request_kind,request.payload]) ? null : previous;
      });
      return response.task;
    } catch (cause) {
      if (token !== generation.current || latest.current.key !== key || responseBoundary?.owner!==delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner) return;
      setReadError({ key, message: cause instanceof Error ? cause.message : String(cause) });
      if (confirm) throw cause;
    } finally {
      if (token === generation.current) { readPending.current = false; setLoading(false); }
    }
  };
  useEffect(() => {
    void read(true);
    return () => { generation.current++; readPending.current = false; };
  }, [key, revision, writable]);
  useEffect(() => { mounted.current = true; actionPending.current = false; setAction(null); return () => { mounted.current = false; actionPending.current = false; }; }, [key]);
  const currentHistory = history?.key === key ? history : null;
  const currentTask = currentHistory?.revision === revision ? currentHistory.task : task;
  const error = readError?.key === key ? readError.message : null;
  const mutationError = actionError?.key === key ? actionError.message : null;
  // Capture the displayed owner's lease with this render's callbacks. Same
  // pane/thread strings must not let an obsolete callback acquire a new Entry.
  const renderOwner=delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner;
  const ownsMutation=()=>mounted.current && renderOwner!==undefined && renderOwner===delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner && latest.current.key===key && latest.current.writable && isDelegationPaneOwner(paneId,task.parent_thread_id);
  const canAct = writable && renderOwner!==undefined && currentHistory?.revision===revision && currentHistory.writable && qualifiesDelegationResponseRead(paneId,task.parent_thread_id,task.id,currentHistory.responseBoundary);
  const mutate = async (kind: 'stop' | 'deliver' | 'respond', requestId?: string, decision?: ApprovalDecision, retry = false, displayedRequest?: RemoteApproval) => {
    if (!ownsMutation() || !canAct || delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.phase?.pending || actionPending.current || loading || latest.current.revision !== revision) return;
    const snapshot = historyRef.current?.key === key && historyRef.current.revision === revision ? historyRef.current.task : latest.current.task;
    if ((kind === 'stop' && !isRemoteTaskActive(snapshot)) || (kind === 'deliver' && !snapshot.delivery?.eligible) || (kind === 'respond' && (!requestId || !decision || !snapshot.remote?.pending_requests.some(request => request.request_id === requestId) || snapshot.cancel_requested))) return;
    if (kind === 'respond' && displayedRequest?.request_kind !== 'user-input' && decision?.decision !== 'deny' && (!displayedRequest || !approvalSummary(displayedRequest).supported)) return;
    actionPending.current = true;
    setAction(kind === 'respond' ? requestId! : kind);
    setActionError(null);
    const current = ownsMutation;
    const stop = kind === 'stop' ? { settled: false } : undefined;
    if (stop) stopConfirmation.current = stop;
    try {
      if (kind === 'stop') {
        try { await delegationCancel(paneId, task.id); }
        finally { stop!.settled = true; }
      }
      else if (kind === 'deliver') await delegationDeliver(paneId, task.id);
      else {
        const request=displayedRequest;
        if(!request || request.request_id!==requestId || !snapshot.remote!.pending_requests.some(current=>JSON.stringify(current)===JSON.stringify(request))) return;
        const retained=retainedDelegationResponse(paneId,task.parent_thread_id,task.id,requestId!);
        const projection=snapshot.responses?.find(response=>response.request_id===requestId);
        if(retry) {
          if(historyRef.current?.key!==key || historyRef.current.revision!==revision || !historyRef.current.writable || !qualifiesDelegationResponseRead(paneId,task.parent_thread_id,task.id,historyRef.current.responseBoundary) || !!readError || !projection?.eligible || !['not_attempted','before_send'].includes(projection.outcome)) return;
          if(retained && retained.payloadIdentity!==JSON.stringify([request.request_kind,request.payload])) return;
          // Native durable decision wins; not-attempted retries need the exact
          // locally retained decision, never a reconstructed/edited form.
          decision=projection.outcome==='before_send' ? projection.decision ?? undefined : retained?.decision;
          if(!decision) return;
        } else if(retained || projection?.decision || (projection && projection.outcome!=='not_attempted')) return;
        retainDelegationResponse(paneId,task.parent_thread_id,task.id,request,decision!);
        const settle=beginDelegationResponse(paneId,task.parent_thread_id,task.id);
        try { await delegationRespond(paneId, task.id, requestId!, decision!, request); }
        finally { settle(); } // Before success readback, and on every rejection.
      }
      if (!current()) return;
      if (kind === 'stop') setStopRequested(key);
      const confirmed = await read(true, true);
      if (!current()) return;
      // An invalidation/revision read now owns confirmation. Discarding this
      // older read is not a failed Stop transport or remote cancellation.
      if (!confirmed && kind === 'stop') return;
      if (!confirmed) throw new Error('The task changed before readback completed. Refresh its saved state before retrying.');
      upsertDelegationTask(paneId, task.parent_thread_id, confirmed);
    } catch (cause) {
      if (current()) setActionError({ key, stop, requestId:kind==='respond'?requestId:undefined, message: `${kind === 'stop' ? 'Stop' : kind === 'deliver' ? 'Parent delivery' : 'Approval response'} unconfirmed: ${cause instanceof Error ? cause.message : String(cause)}. Refresh this task before retrying; no replacement task is started.` });
    } finally {
      if (mounted.current && latest.current.key===key && renderOwner!==undefined && renderOwner===delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.owner) { actionPending.current = false; setAction(null); }
    }
  };
  return <section className="flex min-h-0 flex-1 flex-col" aria-label="Remote task detail" onKeyDown={event => {
    if (event.key === 'Escape' && !(event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement || event.target instanceof HTMLSelectElement)) { event.preventDefault(); onBack(); }
  }}>
    <nav aria-label="Task breadcrumb" className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2">
      <Button type="button" size="sm" variant="ghost" aria-label="Back to parent conversation" onClick={onBack}><ArrowLeft aria-hidden className="size-3.5" />Parent conversation</Button>
      <ChevronRight aria-hidden className="size-3.5 text-muted-foreground" /><span className="truncate text-body-sm font-medium">{currentTask.title || 'Delegated task'}</span>
    </nav>
    <div className="min-h-0 overflow-y-auto p-4">
      <div className="mx-auto grid max-w-3xl gap-5">
        <header className="grid gap-3"><div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-body-lg font-medium">{currentTask.title || 'Delegated task'}</h2><RemoteTaskStatus task={currentTask} /></div>
          <div className="grid gap-1 rounded-lg border border-border bg-surface-1 p-3 text-body-sm"><p className="flex items-center gap-2 font-medium"><Server aria-hidden className="size-4" /><span>{currentTask.target_host_name}</span></p><p className="break-all text-muted-foreground">{currentTask.target_ssh_target}</p><p className="break-all font-mono">{currentTask.target_workspace_path}</p><p className="text-muted-foreground">{currentTask.provider === 'codex' ? 'Codex' : 'Claude'} · {currentTask.model ?? 'Runtime default model'} · {currentTask.permission_mode}</p></div>
          {currentTask.connection_error && <p role="status" className="text-body-sm text-muted-foreground">Reconnecting. Remote execution may continue; the last confirmed state remains above. {currentTask.connection_error}</p>}
          {currentTask.cancel_requested && ['starting', 'running', 'awaiting_approval', 'stopping'].includes(currentTask.status) && <p role="status" className="text-body-sm text-muted-foreground">Stop requested; awaiting remote confirmation.</p>}
          {!writable && <p className="text-body-sm text-muted-foreground">Read-only conversation · remote actions unavailable.</p>}
          {isRemoteTaskActive(currentTask) && <div className="flex flex-wrap items-center gap-3"><Button type="button" size="sm" variant="outline" aria-label="Stop task" disabled={!canAct || loading || !!action || (!!currentTask.cancel_requested && !mutationError && !currentTask.connection_error)} onClick={() => void mutate('stop')}>{action === 'stop' ? 'Requesting stop…' : currentTask.cancel_requested ? 'Stop requested' : 'Stop task'}</Button>{stopRequested === key && !currentTask.cancel_requested && <p role="status" className="text-body-sm text-muted-foreground">Stop requested; awaiting remote confirmation.</p>}</div>}
          {mutationError && <p role="alert" className="text-body-sm text-destructive">{mutationError}</p>}
        </header>
        {!!currentTask.remote?.pending_requests.length && <div className="grid gap-3"><h3 className="text-body font-medium">Approval or answer needed</h3>{currentTask.remote.pending_requests.map(request => {
          const retained=retainedDelegationResponse(paneId,task.parent_thread_id,task.id,request.request_id);
          const projection=currentTask.responses?.find(response=>response.request_id===request.request_id);
          const exact=projection?.decision ?? retained?.decision;
          const locked=!!retained || !!projection && projection.outcome!=='not_attempted';
          const responseError=actionError?.key===key && actionError.requestId===request.request_id;
          const payloadMatches=!retained || retained.payloadIdentity===JSON.stringify([request.request_kind,request.payload]);
          const supported=request.request_kind==='user-input' || approvalSummary(request).supported;
          const retryable=!!exact && (supported || exact.decision==='deny') && payloadMatches && !!projection?.eligible && ['not_attempted','before_send'].includes(projection.outcome) && !!currentHistory && currentHistory.revision===revision && currentHistory.writable && qualifiesDelegationResponseRead(paneId,task.parent_thread_id,task.id,currentHistory.responseBoundary);
          return <div key={`${request.request_id}:${JSON.stringify(request.payload)}`} className="grid gap-3 rounded-lg border border-border bg-surface-1 p-3">
            {request.request_kind !== 'user-input' && <RemoteApprovalDetails request={request} task={currentTask} />}
            {request.request_kind==='user-input' && <RemoteQuestion request={request} retained={locked} isCurrent={ownsMutation} disabled={!canAct || loading || !!action || currentTask.cancel_requested || !!error || responseError || projection?.eligible===false} onSubmit={decision=>mutate('respond',request.request_id,decision,false,request)} />}
            {request.request_kind === 'user-input' && <details><summary className="cursor-pointer text-body-sm text-muted-foreground">Full request details</summary><pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-all font-mono text-body-sm">{JSON.stringify(request, null, 2)}</pre></details>}
            <div className="flex flex-wrap items-center gap-2">{request.request_kind!=='user-input' && !locked && <Button type="button" size="sm" aria-label={`Allow request ${request.request_id}`} disabled={!approvalSummary(request).supported || !canAct || loading || !!action || currentTask.cancel_requested || !!error || projection?.eligible===false} onClick={() => void mutate('respond', request.request_id, { decision: 'allow' },false,request)}>Allow</Button>}<Button type="button" size="sm" variant="outline" aria-label={`Deny request ${request.request_id}`} disabled={!canAct || loading || !!action || currentTask.cancel_requested || !!error || responseError || locked || projection?.eligible===false} onClick={() => void mutate('respond', request.request_id, { decision: 'deny', message: 'Denied by user in parent conversation.' },false,request)}>Deny</Button>{action === request.request_id && <p role="status" className="text-body-sm text-muted-foreground">Confirming approval response…</p>}</div>
            {locked && <div className="grid gap-2"><p role="status">Saved immutable response · {projection?.outcome ?? 'unconfirmed'}. {projection?.reason ?? 'Refresh this task to confirm the response outcome.'}</p>{exact && <pre className="whitespace-pre-wrap break-all text-body-sm">{JSON.stringify(exact,null,2)}</pre>}{retryable && <Button type="button" size="sm" aria-label={`Retry exact response ${request.request_id}`} disabled={!canAct || loading || !!action || currentTask.cancel_requested || !!error} onClick={()=>void mutate('respond',request.request_id,exact,true,request)}>Retry exact saved response</Button>}{!retryable && <p className="text-body-sm text-muted-foreground">Refresh only; this response cannot be changed or resent.</p>}</div>}
          </div>;
        })}</div>}
        {['held','suppressed'].includes(currentTask.wake_state) && <div className="grid gap-3 rounded-lg border border-border bg-surface-1 p-3"><h3 className="text-body font-medium">{currentTask.wake_state==='held'?'Result held':'Parent delivery suppressed'}</h3><p className="text-body-sm text-muted-foreground">{deliveryExplanation(currentTask)}</p>{currentTask.wake_error && <p className={`text-body-sm ${canAct && !error && !mutationError && !currentTask.connection_error && !currentTask.remote?.error && currentTask.status === 'cancelled' && currentTask.cancel_requested && currentTask.wake_state === 'suppressed' && currentTask.delivery?.outcome === 'not_attempted' && currentTask.delivery.eligible === false && currentTask.delivery.reason === 'cancelled' ? 'text-muted-foreground' : 'text-destructive'}`}>{currentTask.wake_error}</p>}{currentTask.delivery?.eligible && <div><Button type="button" size="sm" disabled={!canAct || loading || !!action} onClick={() => void mutate('deliver')}>{action === 'deliver' ? 'Confirming delivery…' : 'Deliver to parent'}</Button></div>}</div>}
        {currentTask.wake_state === 'delivering' && <p role="status" className="text-body-sm text-muted-foreground">Parent delivery in progress; confirmation pending.</p>}
        {currentTask.wake_state === 'delivered' && <p className="text-body-sm text-muted-foreground">Delivered to parent</p>}

        <div className="grid gap-2"><h3 className="text-body font-medium">Task</h3><p className="whitespace-pre-wrap break-words text-body">{currentTask.prompt}</p></div>
        {currentTask.remote?.activity && <div className="grid gap-2"><h3 className="text-body font-medium">Progress</h3><p className="whitespace-pre-wrap break-words text-body">{currentTask.remote.activity}</p></div>}
        {currentTask.remote?.result != null && <div className="grid gap-2"><h3 className="text-body font-medium">Result</h3><p className="whitespace-pre-wrap break-words text-body">{currentTask.remote.result}</p></div>}
        {currentTask.remote?.error && <div role="alert" className="grid gap-2 text-destructive"><h3 className="text-body font-medium">Execution error</h3><p className="whitespace-pre-wrap break-words text-body">{currentTask.remote.error}</p></div>}
        <div className="grid gap-3"><div className="flex flex-wrap items-center justify-between gap-2"><h3 className="text-body font-medium">Recorded events</h3><Button type="button" size="sm" variant="ghost" disabled={loading || !!action} onClick={() => void read()}>Refresh task</Button></div>
          <p className="text-body-sm text-muted-foreground">Actual numbered receiver records. Remote paths and links are shown as text, never opened on this device.</p>
          {loading && <p role="status" className="text-body-sm text-muted-foreground">Reading task history…</p>}
          {error && <p role="alert" className="whitespace-pre-wrap text-body-sm text-destructive">Could not read task history: {error}. Use Refresh task to retry; no new task will be launched.</p>}
          {!loading && !error && !currentHistory?.events.length && <p className="text-body-sm text-muted-foreground">No recorded events yet.</p>}
          <ol className="grid gap-3">{currentHistory?.events.map(record => <li key={record.sequence} className="grid gap-2 rounded-lg border border-border bg-surface-1 p-3"><div className="flex flex-wrap items-center gap-2 text-label text-muted-foreground"><span>Event #{record.sequence}</span><span>{record.event.type}</span></div>{eventText(record) != null && <p className="whitespace-pre-wrap break-words text-body">{eventText(record)}</p>}<details><summary className="cursor-pointer text-body-sm text-muted-foreground">Raw event</summary><pre className="mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-all font-mono text-body-sm">{JSON.stringify(record.event, null, 2)}</pre></details></li>)}</ol>
          {currentHistory?.hasMore && <div className="flex flex-wrap items-center gap-3"><Button type="button" variant="outline" size="sm" disabled={loading || !!action || delegationResponseReadBoundary(paneId,task.parent_thread_id,task.id)?.phase?.pending} onClick={() => void read()}>Load more events</Button><span className="text-body-sm text-muted-foreground">More recorded events are available.</span></div>}
        </div>
      </div>
    </div>
  </section>;
}
