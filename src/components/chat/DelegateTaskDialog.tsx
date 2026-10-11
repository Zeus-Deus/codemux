import { useEffect, useId, useRef, useState } from 'react';
import { Server, ShieldCheck } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DIALOG_CRISP_POSITION } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { useHostsStore } from '@/stores/hosts-store';
import { delegateTask, delegationAuthorize, delegationGrants, delegationHostInfo, delegationRead, type DelegateTaskInput, type DelegationHostInfo, type DelegationProvider, type LocalTask } from '@/lib/delegation';
import { isDelegationPaneOwner, upsertDelegationTask } from '@/stores/delegation-store';
import { DelegationGrants } from './DelegationGrants';

export interface DelegateTaskDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  paneId: string;
  threadId: string;
  onLaunched: (task: LocalTask) => void;
  writable: boolean;
}
const fieldClass = 'w-full rounded-md border border-border bg-background px-3 py-2 text-body outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50';
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
export function DelegateTaskDialog(props: DelegateTaskDialogProps) {
  const { open, onOpenChange, paneId, threadId, writable } = props;
  const id = useId();
  const hosts = useHostsStore(state => state.hosts);
  const hostError = useHostsStore(state => state.error);
  const hostLoading = useHostsStore(state => state.loading);
  const [hostId, setHostId] = useState('');
  const [workspaceId, setWorkspaceId] = useState('');
  const [provider, setProvider] = useState<DelegationProvider>('codex');
  const [model, setModel] = useState('');
  const [permission, setPermission] = useState('');
  const [effort, setEffort] = useState('');
  const [prompt, setPrompt] = useState('');
  const [consent, setConsent] = useState(false);
  const [receiver, setReceiver] = useState<{ key: string; info: DelegationHostInfo } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [phase, setPhase] = useState<'idle' | 'authorizing' | 'launching' | 'confirming'>('idle');
  const [retrying, setRetrying] = useState(false);
  const operation = useRef(0);
  const pending = useRef(false);
  const attempt = useRef<{ requestId: string; grantId?: string; input?: DelegateTaskInput; taskId?: string; dispatched: boolean } | null>(null);
  const scope = useRef(JSON.stringify([paneId, threadId]));
  const selectedHost = hosts.find(host => String(host.id) === hostId);
  const selectionKey = JSON.stringify([paneId, threadId, open, hostId, selectedHost?.ssh_target, selectedHost?.updated_at, refresh]);
  const latest = useRef({ ...props, selectionKey });
  latest.current = { ...props, selectionKey };
  useEffect(() => {
    const nextScope = JSON.stringify([paneId, threadId]);
    if (scope.current !== nextScope) {
      operation.current++;
      pending.current = false;
      attempt.current = null;
      setRetrying(false);
      setPhase('idle');
      setPrompt('');
      setConsent(false);
      setHostId('');
      setWorkspaceId('');
      scope.current = nextScope;
    }
    return () => { operation.current++; };
  }, [paneId, threadId]);
  useEffect(() => { if (open) void useHostsStore.getState().init(); }, [open]);
  useEffect(() => {
    let current = true;
    setReceiver(null);
    setConsent(false);
    setLoading(false);
    if (!open || !selectedHost) return;
    setLoading(true);
    setError(null);
    void delegationHostInfo(selectedHost.id).then(info => {
      if (!current || latest.current.selectionKey !== selectionKey) return;
      if (info.protocol_version !== 1) throw new Error('This host needs a compatible codemux-remote helper. Update it in Settings → Devices.');
      setReceiver({ key: selectionKey, info });
    }).catch(cause => {
      if (current && latest.current.selectionKey === selectionKey) setError(errorText(cause));
    }).finally(() => { if (current && latest.current.selectionKey === selectionKey) setLoading(false); });
    return () => { current = false; };
  }, [selectionKey]);
  const info = receiver?.key === selectionKey ? receiver.info : null;
  const providerInfo = info?.providers.find(item => item.provider === provider);
  const caps = providerInfo?.error ? null : providerInfo?.capabilities;
  useEffect(() => {
    if (attempt.current?.dispatched) return;
    setModel(caps?.models[0]?.id ?? '');
    setPermission(caps?.permission_modes.find(mode => mode.value === caps.default_permission_mode)?.value ?? caps?.permission_modes.find(mode => mode.is_default)?.value ?? '');
    setConsent(false);
  }, [caps]);
  const selectedModel = caps?.models.find(item => item.id === model);
  useEffect(() => { if (!attempt.current?.dispatched) setEffort(selectedModel?.default_effort ?? ''); }, [selectedModel]);
  const selectedPermission = caps?.permission_modes.find(mode => mode.value === permission);
  const workspace = info?.workspaces.find(item => item.id === workspaceId);
  const valid = writable && isDelegationPaneOwner(paneId, threadId) && (!!attempt.current?.taskId || (!!selectedHost && !!workspace && !!selectedModel && !!selectedPermission && consent && !!prompt.trim() && new TextEncoder().encode(prompt).length <= 32000 && !hostError && !loading));
  const locked = phase !== 'idle' || !!attempt.current?.dispatched;
  const close = (nextOpen: boolean) => {
    if (nextOpen) { onOpenChange(true); return; }
    if (phase === 'launching' || phase === 'confirming') return;
    operation.current++;
    pending.current = false;
    setPhase('idle');
    onOpenChange(false);
  };
  const launch = async () => {
    if (!valid || pending.current) return;
    pending.current = true;
    const token = ++operation.current;
    const active = () => token === operation.current && latest.current.open && latest.current.paneId === paneId && latest.current.threadId === threadId && latest.current.writable && isDelegationPaneOwner(paneId, threadId);
    const assertTarget = () => {
      if (!active()) throw new Error('Conversation changed or is now read-only. No further remote action was sent.');
      // A durable task ID needs only identity-scoped local journal readback.
      if (attempt.current?.taskId) return;
      if (!selectedHost) throw new Error('The selected host is unavailable.');
      const host = useHostsStore.getState().hosts.find(item => item.id === selectedHost.id);
      if (!host || host.ssh_target !== selectedHost.ssh_target || useHostsStore.getState().error) throw new Error('The selected host changed or disappeared. Restore and verify this exact host before retrying.');
    };
    setError(null);
    setPhase('authorizing');
    const currentAttempt = attempt.current ?? { requestId: crypto.randomUUID(), dispatched: false };
    attempt.current = currentAttempt;
    try {
      assertTarget();
      if (!currentAttempt.taskId) {
        if (!selectedHost || !workspace || !selectedModel || !selectedPermission) throw new Error('Receiver launch capabilities are unavailable.');
        const fresh = await delegationHostInfo(selectedHost.id);
        assertTarget();
        const freshProvider = fresh.providers.find(item => item.provider === provider);
        const freshModel = freshProvider?.capabilities?.models.find(item => item.id === model);
        if (fresh.protocol_version !== 1 || !fresh.workspaces.some(item => item.id === workspace.id && item.path === workspace.path) || freshProvider?.error || !freshModel || (!!effort && !freshModel.effort_levels.includes(effort)) || !freshProvider?.capabilities?.permission_modes.some(item => item.value === permission)) throw new Error('The receiver checkout, model or permission is no longer available. Refresh the receiver before running.');
        if (!currentAttempt.grantId) {
          const grant = await delegationAuthorize(paneId, { host_id: selectedHost.id, workspace_path: workspace.path, provider, permission_mode: permission });
          assertTarget();
          currentAttempt.grantId = grant.id;
        }
        const grants = await delegationGrants(paneId);
        assertTarget();
        if (!grants.some(grant => grant.id === currentAttempt.grantId && grant.enabled && grant.host_id === selectedHost.id && grant.ssh_target === selectedHost.ssh_target && grant.workspace_path === workspace.path && grant.provider === provider && grant.permission_mode === permission)) throw new Error('Remote authorization could not be confirmed. Verify the host and checkout, then retry.');
        currentAttempt.input ??= { target_id: currentAttempt.grantId!, prompt, model, effort: effort || null, client_request_id: currentAttempt.requestId };
        currentAttempt.dispatched = true;
        setPhase('launching');
        const task = await delegateTask(paneId, currentAttempt.input);
        if (task.parent_thread_id !== threadId) throw new Error('The native launch returned a different parent conversation. Refresh tasks; do not start a replacement.');
        currentAttempt.taskId = task.id;
      }
      assertTarget();
      setPhase('confirming');
      const confirmed = await delegationRead(paneId, currentAttempt.taskId!, 0);
      assertTarget();
      if (confirmed.task.id !== currentAttempt.taskId || confirmed.task.parent_thread_id !== threadId) throw new Error('Task readback did not match this conversation. Refresh tasks before retrying.');
      upsertDelegationTask(paneId, threadId, confirmed.task);
      latest.current.onLaunched(confirmed.task);
      attempt.current = null;
      setRetrying(false);
      setPrompt('');
      setConsent(false);
      latest.current.onOpenChange(false);
    } catch (cause) {
      if (token === operation.current) {
        setRetrying(!!currentAttempt.dispatched);
        setError(`${currentAttempt.dispatched ? 'Launch not confirmed. Retry the same task; do not create a replacement. ' : ''}${errorText(cause)}`);
      }
    } finally {
      if (token === operation.current) { pending.current = false; setPhase('idle'); }
    }
  };
  const select = (label: string, value: string, onChange: (value: string) => void, children: React.ReactNode, disabled = false) => (
    <label className="grid gap-1.5 text-body-sm" htmlFor={`${id}-${label}`}><span className="font-medium">{label}</span>
      <select id={`${id}-${label}`} className={fieldClass} value={value} disabled={disabled || !writable || locked} onChange={event => { if (locked) return; attempt.current = null; onChange(event.target.value); setConsent(false); }}>{children}</select>
    </label>
  );
  return <Dialog open={open} onOpenChange={close}>
    <DialogContent showCloseButton={phase !== 'launching' && phase !== 'confirming'} className={`${DIALOG_CRISP_POSITION} max-h-[calc(100dvh-7rem)] overflow-y-auto sm:max-w-xl`}>
      <DialogHeader><DialogTitle className="flex items-center gap-2"><Server className="size-4" aria-hidden />Run on host</DialogTitle>
        <DialogDescription>A self-contained task on another device. Your parent conversation and draft stay here.</DialogDescription></DialogHeader>
      <form className="grid gap-4" onSubmit={event => { event.preventDefault(); void launch(); }}>
        {select('Host', hostId, value => { setHostId(value); setWorkspaceId(''); }, <><option value="">Choose a configured host…</option>{hostId && !selectedHost && <option value={hostId}>Host unavailable</option>}{hosts.map(host => <option key={host.id} value={host.id}>{host.name}</option>)}</>, hostLoading)}
        {selectedHost && <p className="-mt-2 break-all text-body-sm text-muted-foreground">{selectedHost.ssh_target}</p>}
        {hostId && !selectedHost && <p role="alert" className="text-body-sm text-destructive">Host unavailable. Choose a configured host; this task will not run locally.</p>}
        {hostError && <p role="alert" className="text-body-sm text-destructive">Could not verify configured hosts: {hostError}</p>}
        {!hostLoading && !hostError && !hosts.length && <p className="text-body-sm text-muted-foreground">Add a remote host in Settings → Devices, then reopen this dialog. Delegation never falls back to this device.</p>}
        {loading && <p role="status" className="text-body-sm text-muted-foreground">Reading receiver capabilities and checkouts…</p>}
        {select('Existing checkout', workspaceId, setWorkspaceId, <><option value="">Choose an existing checkout…</option>{workspaceId && !workspace && <option value={workspaceId}>Checkout unavailable</option>}{info?.workspaces.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</>, !info)}
        {workspace && <p className="-mt-2 break-all font-mono text-body-sm text-muted-foreground">{workspace.path}</p>}
        {info && !info.workspaces.length && <p className="text-body-sm text-muted-foreground">No existing checkouts reported. Create a checkout on this host, then refresh.</p>}
        {workspaceId && info && !workspace && <p role="alert" className="text-body-sm text-destructive">Checkout unavailable. Choose an existing checkout before running.</p>}
        <div className="grid gap-3 sm:grid-cols-2">
          {select('Provider', provider, value => setProvider(value as DelegationProvider), <><option value="codex">Codex</option><option value="claude">Claude</option></>, !info)}
          {select('Model', model, setModel, <><option value="">Choose receiver model…</option>{caps?.models.map(item => <option key={item.id} value={item.id}>{item.label}</option>)}</>, !caps)}
        </div>
        <p className="-mt-2 text-label text-muted-foreground">Remote delegation currently supports Codex and Claude. Models and permissions come from this host’s installed runtime.</p>
        {info && !caps && <p role="alert" className="text-body-sm text-destructive">{providerInfo?.error ?? `${provider === 'codex' ? 'Codex' : 'Claude'} is unavailable on this host. Install and authenticate it there, then refresh.`}</p>}
        {caps && (!caps.models.length || !caps.permission_modes.length) && <p role="alert" className="text-body-sm text-destructive">No runnable models or permission modes were reported by this receiver. Authenticate the provider on this host and refresh; local defaults are not used.</p>}
        {select('Permission', permission, setPermission, <><option value="">Choose permission…</option>{caps?.permission_modes.map(mode => <option key={mode.value} value={mode.value}>{mode.label}</option>)}</>, !caps)}
        {selectedPermission && <p className="-mt-2 text-body-sm text-muted-foreground">{selectedPermission.description}</p>}
        {!!selectedModel?.effort_levels.length && select('Reasoning effort', effort, setEffort, <><option value="">Runtime default</option>{selectedModel.effort_levels.map(level => <option key={level} value={level}>{caps?.effort_label_map[level] ?? level}</option>)}</>)}
        <label className="grid gap-1.5 text-body-sm" htmlFor={`${id}-task`}><span className="font-medium">Task</span><textarea id={`${id}-task`} className={`${fieldClass} min-h-28 resize-y`} value={prompt} readOnly={locked || !writable} onChange={event => { if (locked || !writable) return; attempt.current = null; setPrompt(event.target.value); }} placeholder="Describe the task and the result you need. Do not include credentials." /></label>
        {new TextEncoder().encode(prompt).length > 32000 && <p role="alert" className="text-body-sm text-destructive">Task exceeds 32,000 UTF-8 bytes. Shorten it before running.</p>}
        <div className="grid gap-2 rounded-lg border border-border bg-surface-1 p-3">
          <p className="flex items-center gap-2 text-body-sm font-medium"><ShieldCheck className="size-4" aria-hidden />Explicit remote access</p>
          <p className="text-body-sm text-muted-foreground">The agent runs as the remote OS user and can access that user’s files and services under the selected permission mode. It may edit this checkout’s working tree. This is not a sandbox. Only this task prompt is forwarded, not your full parent transcript or credentials.</p>
          <label className="flex items-start gap-2 text-body-sm"><input type="checkbox" className="mt-0.5" checked={consent} disabled={!selectedHost || !workspace || !selectedModel || !selectedPermission || !writable || phase !== 'idle'} onChange={event => setConsent(event.target.checked)} /><span>Authorize this host and checkout for {provider === 'codex' ? 'Codex' : 'Claude'} with {selectedPermission?.label ?? 'the selected permission'} and run this task. This grants future delegation from conversations in this workspace, within their permission ceilings. Cancel stops this launch, but does not revoke a grant already saved.</span></label>
        </div>
        {!writable && <p role="status" className="text-body-sm text-muted-foreground">This conversation is read-only. Remote actions are unavailable.</p>}
        {error && <p role="alert" className="text-body-sm text-destructive">{error}</p>}
        {(selectedHost || hostError) && <div><Button type="button" variant="outline" size="sm" disabled={phase !== 'idle' || loading} onClick={() => { if (hostError) void useHostsStore.getState().refresh(); setRefresh(value => value + 1); }}>Refresh receiver</Button></div>}
        {phase !== 'idle' && <p role="status" className="text-body-sm text-muted-foreground">{phase === 'authorizing' ? 'Verifying receiver and authorization…' : phase === 'launching' ? 'Launching remote task…' : 'Confirming saved task…'}</p>}
        <DelegationGrants paneId={paneId} threadId={threadId} writable={writable && !locked} />
        <DialogFooter><Button type="button" variant="ghost" disabled={phase === 'launching' || phase === 'confirming'} onClick={() => close(false)}>Cancel</Button><Button type="submit" disabled={!valid || phase !== 'idle'}>{retrying ? 'Retry same task' : 'Run on host'}</Button></DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
