import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { delegationResponseReadBoundary, retainedDelegationResponse, useDelegationTasks } from '@/stores/delegation-store';
import { remoteFixture, taskFixture } from '@/lib/delegation.test-fixtures';
import { readFileSync } from 'node:fs';
import type { LocalTask, RemoteApproval, TaskRead } from '@/lib/delegation';
const native = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
async function component() {
  const modules = import.meta.glob('./RemoteTaskDetail.tsx');
  expect(modules['./RemoteTaskDetail.tsx'], 'remote detail must exist').toBeTypeOf('function');
  return (await modules['./RemoteTaskDetail.tsx']() as typeof import('./RemoteTaskDetail')).RemoteTaskDetail;
}
// Source-faithful wire fixture only: no SDK/provider/native execution. Bind the
// emitter -> parser -> raw translator -> worker projection to the actual source
// so changing task.provider or inventing a metadata envelope cannot qualify it.
function claudeRawApprovalFixture(toolName: string, kind: string, rawSidecarInput: Record<string, unknown>, requestId = 'raw-tool'): RemoteApproval {
  const source = (path: string) => readFileSync(`${process.cwd()}/${path}`, 'utf8');
  expect(source('sidecar/claude-agent/src/permissions.ts')).toContain('emit.notification("request-opened", {\n      threadId,\n      requestId,\n      toolName,\n      toolInput,');
  expect(source('src-tauri/src/agent_provider/claude/protocol.rs')).toContain('tool_input: field_value(&params, "toolInput")');
  expect(source('src-tauri/src/agent_provider/claude/translate.rs')).toContain('payload: tool_input,');
  expect(source('src-tauri/src/remote/tasks/worker.rs')).toContain('payload: payload.clone(),');
  const sidecarNotification = { method: 'request-opened', params: { threadId: 'child-1', requestId, toolName, toolInput: rawSidecarInput, toolUseId: 'opaque-tool-use', kind } };
  const parsed = { request_id: sidecarNotification.params.requestId, kind: sidecarNotification.params.kind, tool_input: sidecarNotification.params.toolInput };
  const translated = { request_id: parsed.request_id, request_kind: parsed.kind, payload: parsed.tool_input };
  return { request_id: translated.request_id, request_kind: translated.request_kind, payload: translated.payload };
}

it('P3 cancelled delivery notice detail preserves the full diagnostic as muted information after authoritative readback', async () => {
  const Detail = await component();
  const task = taskFixture({ status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' }, remote: remoteFixture({ status: 'cancelled', cancel_requested: true }) });
  const original = JSON.stringify(task);
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  const notice = screen.getByText(task.wake_error!, { normalizer: text => text });
  expect(notice.textContent).toBe(task.wake_error);
  expect(notice, 'P3: confirmed stopped no-attempt suppression is informational').toHaveClass('text-muted-foreground');
  expect(notice).not.toHaveClass('text-destructive');
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(screen.getByText('Parent delivery suppressed')).toBeInTheDocument();
  expect(screen.getByText('Task was cancelled; parent delivery is unavailable.')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Stop task|Deliver to parent|Retry/ })).not.toBeInTheDocument();
  view.rerender(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  expect(JSON.stringify(task)).toBe(original);
  expect(native.invoke.mock.calls).toHaveLength(2);
  expect(native.invoke).toHaveBeenCalledWith('delegation_list', { paneId: 'pane' });
  expect(native.invoke).toHaveBeenCalledWith('delegation_read', { paneId: 'pane', taskId: task.id, cursor: 0 });
});

it.each<{ qualification: string; patch: Partial<LocalTask>; writable?: boolean; paneId?: string; informational?: boolean; failedRead?: boolean }>([
  { qualification: 'unknown outcome', patch: { delivery: { outcome: 'unknown', eligible: false, reason: 'cancelled' } } },
  { qualification: 'in-flight outcome', patch: { delivery: { outcome: 'in_flight', eligible: false, reason: 'cancelled' } } },
  { qualification: 'accepted outcome', patch: { delivery: { outcome: 'accepted', eligible: false, reason: 'cancelled' } } },
  { qualification: 'before-send failure', patch: { delivery: { outcome: 'before_send', eligible: false, reason: 'cancelled' } } },
  { qualification: 'held presentation', patch: { wake_state: 'held' } },
  { qualification: 'missing legacy ledger', patch: { delivery: undefined } },
  { qualification: 'unconfirmed Stop', patch: { status: 'stopping' } },
  { qualification: 'no cancellation intent', patch: { cancel_requested: false } },
  { qualification: 'failed task', patch: { status: 'failed' } },
  { qualification: 'interrupted task', patch: { status: 'interrupted' } },
  { qualification: 'eligible delivery', patch: { delivery: { outcome: 'not_attempted', eligible: true, reason: 'cancelled' } } },
  { qualification: 'different structured reason', patch: { delivery: { outcome: 'not_attempted', eligible: false, reason: 'no_result' } } },
  { qualification: 'read-only conversation', patch: {}, writable: false },
  { qualification: 'foreign pane owner', patch: {}, paneId: 'foreign-pane' },
  { qualification: 'genuine receiver error', patch: { remote: remoteFixture({ status: 'cancelled', error: 'Actual receiver failure' }) } },
  { qualification: 'genuine connection failure', patch: { connection_error: 'Actual connection failure' } },
  { qualification: 'different diagnostic prose', patch: { wake_error: 'Complete unrelated diagnostic\nwith every original detail' }, informational: true },
  { qualification: 'failed authoritative read', patch: {}, failedRead: true },
])('P3 cancelled delivery notice detail keeps exact diagnostic severity for $qualification', async ({ patch, writable = true, paneId = 'pane', informational = false, failedRead = false }) => {
  const Detail = await component();
  const task = taskFixture({ status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' }, remote: remoteFixture({ status: 'cancelled', cancel_requested: true }), ...patch });
  const original = JSON.stringify(task);
  const event = { sequence: 1, event: { type: 'runtime_warning' as const, thread_id: 'child-1', message: 'Original journal diagnostic', original_payload: { wake_error: task.wake_error, delivery: task.delivery ?? null } } };
  native.invoke.mockImplementation(cmd => cmd === 'delegation_read' && failedRead ? Promise.reject(new Error('Authoritative read failed')) : Promise.resolve(cmd === 'delegation_read' ? { task, events: [event], next_cursor: 1, has_more: false } : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId={paneId} writable={writable} onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  const notice = screen.getByText(task.wake_error!, { normalizer: text => text });
  expect(notice.textContent).toBe(task.wake_error);
  expect(notice).toHaveClass(informational ? 'text-muted-foreground' : 'text-destructive');
  expect(notice).not.toHaveClass(informational ? 'text-destructive' : 'text-muted-foreground');
  if (task.remote?.error) expect(screen.getByText(task.remote.error).closest('[role=alert]')).toHaveClass('text-destructive');
  if (failedRead) expect(screen.getByRole('alert')).toHaveTextContent('Authoritative read failed');
  else {
    const raw = screen.getByText('Raw event').closest('details')!.querySelector('pre')!;
    expect(JSON.parse(raw.textContent!)).toEqual(event.event);
    expect(screen.getByText('Event #1')).toBeInTheDocument();
  }
  expect(JSON.stringify(task)).toBe(original);
  expect(view.container.querySelector('a,img')).toBeNull();
  expect(native.invoke.mock.calls.filter(([cmd]) => /cancel|deliver|respond|launch/.test(cmd))).toHaveLength(0);
});

it('P3 cancelled delivery notice detail never mutes an actual unconfirmed delivery mutation error', async () => {
  const Detail = await component();
  const task = taskFixture({ status: 'completed', wake_state: 'held', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: true, reason: null }, remote: remoteFixture({ status: 'completed', result: 'Original complete result' }) });
  let saved = task;
  native.invoke.mockImplementation(cmd => cmd === 'delegation_deliver' ? Promise.reject(new Error('Actual delivery transport failure')) : Promise.resolve(cmd === 'delegation_read' ? { task: saved, events: [], next_cursor: 0, has_more: false } : [saved]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Deliver to parent' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Deliver to parent' }));
  const alert = await screen.findByRole('alert');
  const fullError = alert.textContent;
  saved = { ...task, status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' } };
  view.rerender(<Detail task={saved} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.getByRole('alert').textContent).toBe(fullError);
  expect(screen.getByRole('alert')).toHaveClass('text-destructive');
  expect(screen.getByText(saved.wake_error!)).toHaveClass('text-destructive');
  expect(screen.getByText('Original complete result')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Deliver to parent' })).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_deliver')).toEqual([['delegation_deliver', { paneId: 'pane', taskId: task.id }]]);
  expect(native.invoke.mock.calls.filter(([cmd]) => /cancel|respond|launch/.test(cmd))).toHaveLength(0);
});

it('P3 cancelled delivery notice detail uses the actual readback ledger and retains a later genuine read error', async () => {
  const Detail = await component();
  const task = taskFixture({ status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' } });
  let resolveRead!: (read: TaskRead) => void;
  native.invoke.mockImplementation(cmd => cmd === 'delegation_read' ? new Promise<TaskRead>(resolve => { resolveRead = resolve; }) : Promise.resolve([task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  const notice = screen.getByText(task.wake_error!);
  expect(notice).toHaveClass('text-destructive');
  const unknown = { ...task, delivery: { outcome: 'unknown' as const, eligible: false, reason: 'cancelled' as const } };
  await act(async () => resolveRead({ task: unknown, events: [], next_cursor: 0, has_more: false }));
  expect(screen.getByText(task.wake_error!)).toHaveClass('text-destructive');
  expect(screen.getByText('Parent delivery outcome is unknown. Resending is forbidden.')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await act(async () => resolveRead({ task, events: [], next_cursor: 0, has_more: false }));
  expect(screen.getByText(task.wake_error!)).toHaveClass('text-muted-foreground');
  native.invoke.mockImplementation(cmd => cmd === 'delegation_read' ? Promise.reject(new Error('Later authoritative read failure')) : Promise.resolve([task]));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Later authoritative read failure');
  expect(screen.getByText(task.wake_error!)).toHaveClass('text-destructive');
  expect(native.invoke.mock.calls.filter(([cmd]) => /cancel|deliver|respond|launch/.test(cmd))).toHaveLength(0);
});

it.each([
  { query: 'Remote tool input' },
  { input: 'This is a real raw argument, not an envelope' },
  { tool_input: ['This is also a raw argument'], description: 'Raw tool description' },
  { tool_name: 'Do not infer identity', query: 'Keep every original field', file_path: 17, cwd: 'file:///srv/raw', grantRoot: null },
  {},
])('FEI-2 generic raw sidecar input has honest complete consent and exact Allow without inferred metadata: %j', async rawSidecarInput => {
  const Detail = await component();
  const request = claudeRawApprovalFixture(Object.keys(rawSidecarInput).length ? 'WebSearch' : 'GetStatus', 'other', rawSidecarInput);
  const task = taskFixture({ provider: 'claude', status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Allow request raw-tool' })).toBeEnabled());
  expect(screen.getByText('Use remote tool on Build host')).toBeInTheDocument();
  expect(screen.getByText(/Tool identity is not included in this approval/)).toBeInTheDocument();
  const input = screen.getByLabelText('Original tool input');
  expect(JSON.parse(input.textContent!)).toEqual(rawSidecarInput);
  expect(input.closest('details')).toBeNull();
  expect(screen.getByText('Checkout on Build host: /srv/project')).toBeInTheDocument();
  expect(screen.queryByText(/Requested path on|Requested scope on|Working directory on|Use WebSearch/)).not.toBeInTheDocument();
  expect(view.container.querySelector('a,img')).toBeNull();
  expect(screen.queryByRole('button', { name: /session|always/i })).not.toBeInTheDocument();
  const disclosure = screen.getByText('Full request details').closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  fireEvent.click(input);
  expect(native.invoke.mock.calls.filter(([cmd]) => /open|respond/.test(cmd))).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Allow request raw-tool' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'allow' } }]]));
});

it('FEI-2 generic raw sidecar input restores identical native saved-Allow before-send retry, not a reconstructed request', async () => {
  const Detail = await component();
  const rawSidecarInput = { query: 'Saved original query', input: null, description: 'Original description' };
  const request = claudeRawApprovalFixture('WebSearch', 'other', rawSidecarInput, 'raw-retry');
  const exact = { decision: 'allow' as const, updated_input: { ...rawSidecarInput, extra: 'Immutable native decision' } };
  const task = taskFixture({ provider: 'claude', status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }), responses: [{ request_id: request.request_id, outcome: 'before_send', eligible: true, reason: null, decision: exact }] });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  const retry = await screen.findByRole('button', { name: 'Retry exact response raw-retry' });
  await waitFor(() => expect(retry).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Allow request raw-retry' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Deny request raw-retry' })).toBeDisabled();
  fireEvent.click(retry); fireEvent.click(retry);
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: exact }]]));
});

it('FEI-2 compatibility retains generic Allow after preclaim refusal and retries it only after a post-attempt read', async () => {
  const Detail = await component();
  const request = claudeRawApprovalFixture('GetStatus', 'other', {}, 'raw-local-retry');
  const task = taskFixture({ provider: 'claude', status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }), responses: [{ request_id: request.request_id, outcome: 'not_attempted', eligible: true, reason: null, decision: null }] });
  let fail = true;
  native.invoke.mockImplementation(cmd => cmd === 'delegation_respond' && fail ? Promise.reject(new Error('Before-claim capacity refusal')) : Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const props = { task, paneId: 'pane', writable: true, onBack: vi.fn() };
  let view = render(<Detail {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Allow request raw-local-retry' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Allow request raw-local-retry' }));
  await screen.findByText(/Approval response unconfirmed: Before-claim capacity refusal/);
  expect(screen.queryByRole('button', { name: 'Retry exact response raw-local-retry' })).not.toBeInTheDocument();
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)?.decision).toEqual({ decision: 'allow' });
  view.unmount(); fail = false;
  view = render(<Detail {...props} />);
  const retry = await screen.findByRole('button', { name: 'Retry exact response raw-local-retry' });
  await waitFor(() => expect(retry).toBeEnabled());
  fireEvent.click(retry); fireEvent.click(retry);
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual(Array.from({ length: 2 }, () => ['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'allow' } }])));
});

it.each(['accepted', 'unknown', 'in_flight'] as const)('FEI-2 compatibility generic saved Allow never replays after %s, including a previously displayed retry handler', async outcome => {
  const Detail = await component();
  const request = claudeRawApprovalFixture('WebSearch', 'other', { query: 'Original raw query' }, 'raw-settled');
  const exact = { decision: 'allow' as const };
  const task = taskFixture({ provider: 'claude', status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }), responses: [{ request_id: request.request_id, outcome: 'before_send', eligible: true, reason: null, decision: exact }] });
  let reading = task;
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task: reading, events: [], next_cursor: 0, has_more: false } : [reading]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  const retry = await screen.findByRole('button', { name: 'Retry exact response raw-settled' });
  await waitFor(() => expect(retry).toBeEnabled());
  const propsKey = Object.keys(retry).find(key => key.startsWith('__reactProps'))!;
  const displayed = (retry as unknown as Record<string, { onClick: () => void }>)[propsKey].onClick;
  reading = { ...task, responses: [{ request_id: request.request_id, outcome, eligible: false, reason: outcome, decision: exact }] };
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Retry exact response raw-settled' })).not.toBeInTheDocument();
  const before = [...native.invoke.mock.calls];
  await act(async () => displayed());
  expect(native.invoke.mock.calls).toEqual(before);
  expect(screen.getByRole('button', { name: 'Deny request raw-settled' })).toBeDisabled();
});

it.each(['readonly', 'failed_read', 'cancelled'] as const)('FEI-2 compatibility generic raw input cannot bypass %s approval authority at handler entry', async reason => {
  const Detail = await component();
  const request = claudeRawApprovalFixture('WebSearch', 'other', { query: 'Guarded raw input' });
  const task = taskFixture({ provider: 'claude', status: 'awaiting_approval', cancel_requested: reason === 'cancelled', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => cmd === 'delegation_read' && reason === 'failed_read' ? Promise.reject(new Error('Authoritative read refused')) : Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable={reason !== 'readonly'} onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  const allow = screen.getByRole('button', { name: 'Allow request raw-tool' });
  expect(allow).toBeDisabled();
  const propsKey = Object.keys(allow).find(key => key.startsWith('__reactProps'))!;
  const before = [...native.invoke.mock.calls];
  await act(async () => (allow as unknown as Record<string, { onClick: () => void }>)[propsKey].onClick());
  expect(native.invoke.mock.calls).toEqual(before);
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)).toBeUndefined();
});

it('reads numbered real events with honest pagination, safe remote text and parent back navigation', async () => {
  const Detail = await component();
  const task = taskFixture({ status: 'completed', wake_state: 'delivered', remote: remoteFixture({ status: 'completed', result: 'Changed /srv/project/src/routes.ts. [remote file](file:///srv/project/src/routes.ts)' }) });
  native.invoke.mockImplementation((command, args) => {
    if (command !== 'delegation_read') return Promise.resolve([task]);
    const read: TaskRead = args.cursor === 0 ? { task, events: [{ sequence: 41, event: { type: 'item_completed', thread_id: 'child-1', turn_id: 'turn-1', item: { kind: 'assistant_text', text: 'Actual provider report' } } }], next_cursor: 41, has_more: true } :
      { task, events: [{ sequence: 42, event: { type: 'runtime_warning', thread_id: 'child-1', message: 'Actual warning', original_payload: null } }], next_cursor: 42, has_more: false };
    return Promise.resolve(read);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const back = vi.fn();
  const view = render(<Detail task={task} paneId="pane" writable={false} onBack={back} />);
  await screen.findByText('Actual provider report');
  expect(screen.getByText('Event #41')).toBeInTheDocument();
  expect(screen.getByText('/srv/project')).toBeInTheDocument();
  expect(screen.getByText('Build host')).toBeInTheDocument();
  expect(view.container.querySelector('a, img')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Load more events' }));
  await screen.findByText('Actual warning');
  expect(screen.getByText('Event #42')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Load more events' })).not.toBeInTheDocument();
  expect(native.invoke).toHaveBeenCalledWith('delegation_read', { paneId: 'pane', taskId: 'task-1', cursor: 41 });
  expect(native.listen).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Back to parent conversation' }));
  expect(back).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_list')).toHaveLength(1));
});

it.each(['allow', 'deny'] as const)('responds to the actual pending approval with %s and verifies its resolution', async decision => {
  const Detail = await component();
  let task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', pending_requests: [{ request_id: 'request-9', request_kind: 'permission', payload: { command: 'git diff /srv/project' } }] }) });
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_respond') {
      task = taskFixture({ remote: remoteFixture(), updated_at: '2026-10-08T10:01:00Z' });
      return Promise.resolve(task);
    }
    if (command === 'delegation_read') return Promise.resolve({ task, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([task]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: `${decision === 'allow' ? 'Allow' : 'Deny'} request request-9` })).toBeEnabled());
  expect(screen.getByText('git diff /srv/project', { selector: 'pre' })).toBeInTheDocument();
  const button = screen.getByRole('button', { name: `${decision === 'allow' ? 'Allow' : 'Deny'} request request-9` });
  fireEvent.click(button);
  fireEvent.click(button);
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Allow request request-9' })).not.toBeInTheDocument());
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: 'task-1', requestId: 'request-9', originalRequest:{request_id:'request-9',request_kind:'permission',payload:{command:'git diff /srv/project'}}, decision: decision === 'allow' ? { decision: 'allow' } : { decision: 'deny', message: 'Denied by user in parent conversation.' } }]]);
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_read')).toHaveLength(2);
});

it.each(['codex', 'claude'] as const)('presents %s command approval as passive human-readable consent with collapsed complete request and exact outgoing identity', async provider => {
  const Detail = await component();
  const command = 'printf fixture && git diff -- /srv/remote/file.ts';
  const payload = provider === 'codex' ? { command, cwd: '/srv/remote', reason: 'Inspect the requested remote change', threadId: 'opaque-thread', turnId: 'opaque-turn', itemId: 'opaque-item', extra: { flag: false, value: null } } : { command, cwd: '/srv/remote', description: 'Inspect the requested remote change', extra: [null, false] };
  const request = provider === 'claude' ? claudeRawApprovalFixture('Bash', 'command', payload, 'opaque/request-01') : { request_id: 'opaque/request-01', request_kind: 'command', payload };
  const task = taskFixture({ provider, status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  const allow = await screen.findByRole('button', { name: 'Allow request opaque/request-01' });
  await waitFor(() => expect(allow).toBeEnabled());
  expect(screen.getByText('Run command on Build host')).toBeInTheDocument();
  expect(screen.getByText(command, { selector: 'pre' })).toBeInTheDocument();
  expect(screen.getByText('Inspect the requested remote change', { selector: 'p' })).toBeInTheDocument();
  expect(screen.getByText('Working directory on Build host: /srv/remote')).toBeInTheDocument();
  const summary = screen.getByText('Full request details', { selector: 'summary' });
  const disclosure = summary.closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  expect(view.container.querySelector('a,img')).toBeNull();
  expect(screen.queryByRole('button', { name: /session|always/i })).not.toBeInTheDocument();
  fireEvent.click(allow);
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'allow' } }]]));
});

it.each([
  { toolName: 'Edit', request_kind: 'file-change', payload: { file_path: '/srv/remote/file.ts', old_string: 'before', new_string: 'after' }, title: 'Change remote files on Build host', path: 'Requested path on Build host: /srv/remote/file.ts' },
  { toolName: 'Read', request_kind: 'file-read', payload: { file_path: 'file:///srv/remote/read.ts' }, title: 'Read remote files on Build host', path: 'Requested path on Build host: file:///srv/remote/read.ts' },
  { toolName: null, request_kind: 'file-change', payload: { grantRoot: '/srv/remote', reason: 'Change only this requested scope', itemId: 'opaque' }, title: 'Change remote files on Build host', path: 'Requested scope on Build host: /srv/remote' },
  { toolName: 'WebSearch', request_kind: 'other', payload: { query: 'Remote tool input' }, title: 'Use remote tool on Build host', path: 'Checkout on Build host: /srv/project' },
  { toolName: null, request_kind: 'command', payload: { command: 'git diff', cwd: '/srv/remote', reason: null }, title: 'Run command on Build host', path: 'Working directory on Build host: /srv/remote' },
  { toolName: 'Read', request_kind: 'file-read', payload: { file_path: '/srv/remote/file.ts', cwd: null, grantRoot: null, reason: null }, title: 'Read remote files on Build host', path: 'Requested path on Build host: /srv/remote/file.ts' },
  { toolName: null, request_kind: 'file-change', payload: { grantRoot: '/srv/remote', reason: null }, title: 'Change remote files on Build host', path: 'Requested scope on Build host: /srv/remote' },
  { toolName: null, request_kind: 'file-read', payload: { file_path: '/srv/remote/file.ts', input: { file_path: '/srv/remote/file.ts', cwd: null } }, title: 'Read remote files on Build host', path: 'Requested path on Build host: /srv/remote/file.ts' },
  { toolName: null, request_kind: 'file-read', payload: { tool_input: { file_path: '/srv/remote/file.ts', cwd: '/srv/remote' }, input: { file_path: '/srv/remote/file.ts', cwd: '/srv/remote' } }, title: 'Read remote files on Build host', path: 'Requested path on Build host: /srv/remote/file.ts' },
  { toolName: null, request_kind: 'file-change', payload: { input: { grantRoot: '/srv/remote' }, cwd: null, reason: null }, title: 'Change remote files on Build host', path: 'Requested scope on Build host: /srv/remote' },
])('presents supported $request_kind approval for $path without turning remote paths into local openers', async ({ toolName, request_kind, payload, title, path }) => {
  const Detail = await component();
  const request = toolName ? claudeRawApprovalFixture(toolName, request_kind, payload, 'supported') : { request_id: 'supported', request_kind, payload };
  const task = taskFixture({ provider: toolName ? 'claude' : 'codex', status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Allow request supported' })).toBeEnabled());
  expect(screen.getByText(title)).toBeInTheDocument();
  const passivePath = screen.getByText(path);
  fireEvent.click(passivePath);
  expect(view.container.querySelector('a,img')).toBeNull();
  expect(native.invoke.mock.calls.filter(([cmd]) => /open|respond/.test(cmd))).toHaveLength(0);
  expect(screen.getByText('Full request details').closest('details')).not.toHaveAttribute('open');
  fireEvent.click(screen.getByRole('button', { name: 'Allow request supported' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'allow' } }]]));
});

async function assertUnsupportedApproval(request_kind: string, payload: unknown) {
  const Detail = await component();
  const request = { request_id: 'invalid-consent', request_kind, payload };
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Deny request invalid-consent' })).toBeEnabled());
  const allow = screen.getByRole('button', { name: 'Allow request invalid-consent' });
  const before = [...native.invoke.mock.calls];
  const propsKey = Object.keys(allow).find(key => key.startsWith('__reactProps'))!;
  await act(async () => (allow as unknown as Record<string, { onClick: () => void }>)[propsKey].onClick());
  // Admission, not disabled HTML, must reject before retention or any IPC.
  expect(native.invoke.mock.calls).toEqual(before);
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)).toBeUndefined();
  expect(allow).toBeDisabled();
  expect(screen.getByText(/Unsupported approval format/)).toBeInTheDocument();
  const disclosure = screen.getByText('Full request details').closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  expect(view.container.querySelector('a,img')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Deny request invalid-consent' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'deny', message: 'Denied by user in parent conversation.' } }]]));
}

it.each([
  { file_path: 17, tool_input: { file_path: '/srv/remote/read.ts' } },
  { file_path: '/srv/visible.ts', tool_input: { file_path: '/srv/different.ts' } },
  { file_path: '/srv/remote/read.ts', input: { file_path: [] } },
  { file_path: '', input: { file_path: '/srv/remote/read.ts' } },
])('FEI-1 path rejects malformed or contradictory present file_path aliases: $file_path/$tool_input/$input', async payload => {
  await assertUnsupportedApproval('file-read', payload);
});

it.each([
  { cwd: 17, tool_input: { cwd: '/srv/remote' } },
  { cwd: '/srv/visible', input: { cwd: '/srv/different' } },
  { cwd: '/srv/remote', tool_input: { cwd: [] } },
  { cwd: '   ' },
])('FEI-1 directory rejects malformed or contradictory cwd aliases: $cwd/$tool_input/$input', async payload => {
  await assertUnsupportedApproval('command', { command: 'git diff', ...payload });
});

it.each([
  { grantRoot: 17, input: { grantRoot: '/srv/remote' } },
  { grantRoot: '/srv/visible', tool_input: { grantRoot: '/srv/different' } },
  { grantRoot: '/srv/remote', input: { grantRoot: false } },
  { grantRoot: '' },
])('FEI-1 scope rejects malformed or contradictory grantRoot aliases even with a valid file path: $grantRoot/$input/$tool_input', async payload => {
  await assertUnsupportedApproval('file-change', { file_path: '/srv/remote/file.ts', ...payload });
});

it.each([
  { tool_input: { file_path: '/srv/remote/file.ts' }, input: [] },
  { tool_input: { file_path: '/srv/remote/file.ts' }, input: null },
  { tool_input: { file_path: '/srv/remote/file.ts' }, input: { file_path: 17 } },
  { tool_input: { file_path: '/srv/remote/file.ts' }, input: { file_path: '/srv/different.ts' } },
  { tool_input: { file_path: '/srv/remote/file.ts', new_string: 'visible' }, input: { file_path: '/srv/remote/file.ts', new_string: 'different' } },
])('FEI-1 input aliases reject an invalid or contradictory alternate instead of selecting tool_input: $input', async payload => {
  await assertUnsupportedApproval('file-change', payload);
});

it.each([
  { request_kind: 'command', payload: null },
  { request_kind: 'command', payload: ['unrecognized command'] },
  { request_kind: 'command', payload: { command: 17 } },
  { request_kind: 'command', payload: { command: 17, tool_input: { command: 'Do not repair a malformed primary command' } } },
  { request_kind: 'command', payload: { command: 'Displayed primary command', tool_input: { command: 'Different command' } } },
  { request_kind: 'command', payload: { tool_input: [], input: { command: 'Do not repair a malformed primary input' } } },
  { request_kind: 'permission', payload: { tool_name: 'Bash', tool_input: { command: 17 } } },
  { request_kind: 'unknown', payload: { command: 'Not a supported request kind' } },
  { request_kind: 'permissions', payload: { permissions: { network: true } } },
  { request_kind: 'tool-call', payload: { name: 'Not an implemented dynamic tool execution schema', arguments: {} } },
  { request_kind: 'permission', payload: { tool_name: 'No authoritative tool identity', tool_input: {} } },
  { request_kind: 'other', payload: null },
  { request_kind: 'other', payload: [] },
  { request_kind: 'other', payload: 'Not the raw object contract' },
  { request_kind: 'file-read', payload: { file_path: null } },
  { request_kind: 'file-read', payload: { file_path: null, input: { file_path: '/srv/remote/file.ts' } } },
  { request_kind: 'file-change', payload: { grantRoot: null, reason: null } },
])('keeps unsupported remote approval $request_kind/$payload fail-closed while preserving exact Deny identity', async specimen => {
  const Detail = await component();
  const request = { ...specimen, request_id: 'unsupported' };
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Deny request unsupported' })).toBeEnabled());
  expect(screen.getByText(/Unsupported approval format/)).toBeInTheDocument();
  const allow = screen.getByRole('button', { name: 'Allow request unsupported' });
  expect(allow).toBeDisabled();
  // Exercise the real rendered handler too; disabled DOM is not the boundary.
  const propsKey = Object.keys(allow).find(key => key.startsWith('__reactProps'))!;
  await act(async () => (allow as unknown as Record<string, { onClick: () => void }>)[propsKey].onClick());
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toHaveLength(0);
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)).toBeUndefined();
  const disclosure = screen.getByText('Full request details').closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  fireEvent.click(screen.getByRole('button', { name: 'Deny request unsupported' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'deny', message: 'Denied by user in parent conversation.' } }]]));
});

it('keeps an unsupported native saved Allow immutable and confirmation-only instead of advertising a nonfunctional retry', async () => {
  const Detail = await component();
  const request = { request_id: 'unsupported-retry', request_kind: 'unknown', payload: { command: 'Not a supported schema' } };
  const exact = { decision: 'allow' as const };
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ pending_requests: [request] }), responses: [{ request_id: request.request_id, outcome: 'before_send', eligible: true, reason: null, decision: exact }] });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Retry exact response unsupported-retry' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Deny request unsupported-retry' })).toBeDisabled();
  expect(screen.getByText(JSON.stringify(exact, null, 2), { selector: 'pre', normalizer: text => text })).toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toHaveLength(0);
});

it.each(['codex', 'claude'] as const)('native question presentation makes the %s answer form primary and keeps the complete original request in a closed passive disclosure', async provider => {
  const Detail = await component();
  const payload = {
    questions: [{ question: 'Which route?', header: 'Route', options: [{ label: 'Safe', description: 'Keep the remote boundary explicit' }] }, { question: 'Why?', options: [] }],
    remote_hint: 'Original opaque payload '.repeat(800),
    extra: { value: null, flag: false, path: 'file:///srv/remote/original.ts' },
  };
  const request = provider === 'claude' ? claudeRawApprovalFixture('AskUserQuestion', 'user-input', payload, 'opaque/question-01') : { request_id: 'opaque/question-01', request_kind: 'user-input', payload };
  const task = taskFixture({ provider, status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  const formGroup = screen.getByRole('group', { name: 'Answer remote questions' });
  const requestCard = formGroup.parentElement!;
  expect(requestCard.firstElementChild, 'raw identity/payload must not precede the answer form').toBe(formGroup);
  const summary = screen.getByText('Full request details', { selector: 'summary' });
  const disclosure = summary.closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(formGroup.compareDocumentPosition(disclosure) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect([...requestCard.querySelectorAll('pre')].every(pre => pre.closest('details') === disclosure)).toBe(true);
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  expect(disclosure.querySelector('input,textarea,button,[contenteditable="true"],a,img')).toBeNull();
  expect(screen.queryByRole('button', { name: /Allow request/ })).not.toBeInTheDocument();
  expect(screen.getByText('Which route?')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toHaveLength(0);
  fireEvent.click(screen.getByTestId('aq-option-0-0'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  const answer = screen.getByPlaceholderText('Your answer…');
  fireEvent.change(answer, { target: { value: 'Keep the answer explicit' } });
  const beforeInspect = [...native.invoke.mock.calls];
  await act(async () => summary.click());
  expect(disclosure).toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  expect(screen.getByPlaceholderText('Your answer…')).toBe(answer);
  expect(answer).toHaveValue('Keep the answer explicit');
  await act(async () => summary.click());
  expect(disclosure).not.toHaveAttribute('open');
  expect(native.invoke.mock.calls).toEqual(beforeInspect);
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)).toBeUndefined();
  const send = screen.getByRole('button', { name: 'Send' });
  fireEvent.click(send); fireEvent.click(send);
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'allow', updated_input: { ...payload, answers: { 'Which route?': 'Safe', 'Why?': 'Keep the answer explicit' } } } }]]));
  expect(view.container.querySelector('a,img')).toBeNull();
});

it.each([
  null,
  { questions: [] },
  { questions: 'Not the original question array schema' },
  { questions: [{ question: 'Recognized question?', options: [] }, { question: 17, options: [] }] },
  { questions: [{ question: 'Duplicate?', options: [] }, { question: 'Duplicate?', options: [] }] },
])('native question presentation retains original unsupported question schema fail-closed with inspectable exact Deny: %j', async payload => {
  const Detail = await component();
  const request = { request_id: 'unsupported-question', request_kind: 'user-input', payload };
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', pending_requests: [request] }) });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : cmd === 'delegation_respond' ? task : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Deny request unsupported-question' })).toBeEnabled());
  const unsupported = screen.getByText('Unsupported question format. No unanswered Allow will be sent.');
  expect(unsupported.parentElement!.firstElementChild).toBe(unsupported);
  expect(screen.queryByRole('button', { name: /Allow request|Send|Next/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  const summary = screen.getByText('Full request details', { selector: 'summary' });
  const disclosure = summary.closest('details')!;
  expect(disclosure).not.toHaveAttribute('open');
  expect(JSON.parse(disclosure.querySelector('pre')!.textContent!)).toEqual(request);
  const beforeInspect = [...native.invoke.mock.calls];
  await act(async () => summary.click());
  expect(disclosure).toHaveAttribute('open');
  expect(native.invoke.mock.calls).toEqual(beforeInspect);
  expect(retainedDelegationResponse('pane', 'thread-1', task.id, request.request_id)).toBeUndefined();
  fireEvent.click(screen.getByRole('button', { name: 'Deny request unsupported-question' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_respond')).toEqual([['delegation_respond', { paneId: 'pane', taskId: task.id, requestId: request.request_id, originalRequest: request, decision: { decision: 'deny', message: 'Denied by user in parent conversation.' } }]]));
});

it('native question presentation leaves opaque durable activity and complete numbered receiver history inspectable in detail', async () => {
  const Detail = await component();
  const request = { request_id: 'history-question', request_kind: 'user-input', payload: { questions: [{ question: 'Original question?', options: [] }] } };
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', pending_requests: [request] }) });
  const events: TaskRead['events'] = [{ sequence: 19, event: { type: 'runtime_warning', thread_id: 'child-1', message: 'Original receiver diagnostic', original_payload: { type: 'stream_event', event: { type: 'message_stop' } } } }];
  const original = JSON.stringify({ task, events });
  native.invoke.mockImplementation(cmd => Promise.resolve(cmd === 'delegation_read' ? { task, events, next_cursor: 19, has_more: false } : [task]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await screen.findByText('Original receiver diagnostic');
  expect(screen.getByText('stream_event message_stop')).toBeInTheDocument();
  expect(screen.getByText('Progress')).toBeInTheDocument();
  expect(screen.getByText('Event #19')).toBeInTheDocument();
  const raw = screen.getByText('Raw event', { selector: 'summary' });
  expect(JSON.parse(raw.closest('details')!.querySelector('pre')!.textContent!)).toEqual(events[0].event);
  const beforeInspect = [...native.invoke.mock.calls];
  await act(async () => raw.click());
  expect(raw.closest('details')).toHaveAttribute('open');
  expect(native.invoke.mock.calls).toEqual(beforeInspect);
  expect(JSON.stringify({ task, events })).toBe(original);
  expect(view.container.querySelector('a,img')).toBeNull();
});

it('requires structured remote AskUserQuestion answers and submits the original fields with exact allow.updated_input once', async () => {
  const Detail=await component();
  const payload={questions:[{question:'Which route?',header:'Route',multiSelect:false,options:[{label:'Safe',description:'Use /srv/remote',preview:'[remote](file:///srv/remote)'}]},{question:'Why?',options:[]}],remote_hint:'retain literal fields'};
  let task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[{request_id:'ask',request_kind:'user-input',payload}]})});
  native.invoke.mockImplementation(command=>{
    if(command==='delegation_respond'){task=taskFixture({remote:remoteFixture(),updated_at:'settled'});return Promise.resolve(task);}
    return Promise.resolve(command==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:[task]);
  });
  renderHook(()=>useDelegationTasks('pane','thread-1'));
  const view=render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  expect(screen.queryByRole('button',{name:'Allow request ask'})).not.toBeInTheDocument();
  expect(screen.getByRole('button',{name:'Next'})).toBeDisabled();
  fireEvent.click(screen.getByTestId('aq-option-0-0'));
  fireEvent.click(screen.getByRole('button',{name:'Next'}));
  expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
  fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Keep access explicit'}});
  const send=screen.getByRole('button',{name:'Send'});
  fireEvent.click(send); fireEvent.click(send);
  await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toEqual([['delegation_respond',{paneId:'pane',taskId:'task-1',requestId:'ask',originalRequest:{request_id:'ask',request_kind:'user-input',payload},decision:{decision:'allow',updated_input:{...payload,answers:{'Which route?':'Safe','Why?':'Keep access explicit'}}}}]]));
  expect(view.container.querySelector('a,img')).toBeNull();
});
it.each(['readonly','read-failure','response-failure'] as const)('keeps structured question authority fail-closed for %s', async reason=>{
  const Detail=await component();
  const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[{request_id:'ask',request_kind:'user-input',payload:{questions:[{question:'Which route?',options:[{label:'Safe'}]}]}}]})});
  native.invoke.mockImplementation(command=>{
    if(command==='delegation_respond')return Promise.reject(new Error('Answer transport lost'));
    if(command==='delegation_read')return reason==='read-failure'?Promise.reject(new Error('Stale local authority')):Promise.resolve({task,events:[],next_cursor:0,has_more:false});
    return Promise.resolve([task]);
  });
  renderHook(()=>useDelegationTasks('pane','thread-1'));
  render(<Detail task={task} paneId="pane" writable={reason!=='readonly'} onBack={vi.fn()} />);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  expect(screen.queryByRole('button',{name:'Allow request ask'})).not.toBeInTheDocument();
  if(reason==='response-failure'){
    fireEvent.click(screen.getByTestId('aq-option-0-0'));
    fireEvent.click(screen.getByRole('button',{name:'Send'}));
    await screen.findByText(/Approval response unconfirmed: Answer transport lost/);
    expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
    expect(screen.getByRole('radio',{name:/Safe/})).toBeDisabled();
    fireEvent.click(screen.getByRole('button',{name:'Send'}));
    expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
  } else {
    expect(screen.getByRole('radio',{name:/Safe/})).toBeDisabled();
    expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
    expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(0);
  }
});
it.each(['preclaim','before_send'] as const)('FEUI-3 retains the exact answer through %s failure, qualified read and detail reopen',async failure=>{
 const Detail=await component();const payload={questions:[{question:'Recovery answer?',options:[]}],hint:'original'};
 let task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[{request_id:'recover',request_kind:'user-input',payload}]})});
 let fail=true;let qualified=false;
 const exact={decision:'allow',updated_input:{...payload,answers:{'Recovery answer?':'Exact immutable answer'}}};
 native.invoke.mockImplementation(cmd=>{
  if(cmd==='delegation_respond') {
   if(fail && failure==='preclaim')return Promise.reject(new Error('Owned capacity refused before claim'));
   if(fail)return Promise.resolve({...task,connection_error:'Definitely-before-send diagnostic'});
   task=taskFixture({remote:remoteFixture()});return Promise.resolve(task);
  }
  if(cmd==='delegation_read')return Promise.resolve({task:{...task,responses:qualified?[{request_id:'recover',outcome:failure==='preclaim'?'not_attempted':'before_send',decision:failure==='preclaim'?null:exact,eligible:true,reason:null}]:[]},events:[],next_cursor:0,has_more:false});
  return Promise.resolve([task]);
 });
 renderHook(()=>useDelegationTasks('pane','thread-1'));
 let view=render(<Detail task={task} paneId="pane" writable onBack={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Exact immutable answer'}});fireEvent.click(screen.getByRole('button',{name:'Send'}));
 await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 view.unmount();qualified=true;fail=false;
 view=render(<Detail task={task} paneId="pane" writable onBack={vi.fn()}/>);
 await screen.findByText(/Exact immutable answer/,{selector:'pre'});
 const retry=await screen.findByRole('button',{name:'Retry exact response recover'});
 await waitFor(()=>expect(retry).toBeEnabled());
 expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();expect(screen.getByPlaceholderText('Your answer…')).toBeDisabled();
 fireEvent.click(retry);fireEvent.click(retry);
 await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(2));
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond').map(([,args])=>args.decision)).toEqual([exact,exact]);
});
it('FEUI-3 requires a post-attempt qualified read before retry and clears only its response diagnostic',async()=>{
 const Detail=await component();const payload={questions:[{question:'Read-qualified?',options:[]}]};
 const request={request_id:'qualified',request_kind:'user-input',payload};
 const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:'qualified',outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
 let fail=true;
 native.invoke.mockImplementation(cmd=>cmd==='delegation_respond' && fail?Promise.reject(new Error('Capacity unavailable')):Promise.resolve(cmd==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:cmd==='delegation_respond'?task:[task]));
 renderHook(()=>useDelegationTasks('pane','thread-1'));render(<Detail task={task} paneId="pane" writable onBack={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Retained exact'}});fireEvent.click(screen.getByRole('button',{name:'Send'}));
 await screen.findByText(/Approval response unconfirmed: Capacity unavailable/);
 expect(screen.queryByRole('button',{name:'Retry exact response qualified'})).not.toBeInTheDocument();
 fail=false;fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));
 const retry=await screen.findByRole('button',{name:'Retry exact response qualified'});await waitFor(()=>expect(retry).toBeEnabled());
 expect(screen.queryByText(/Approval response unconfirmed: Capacity unavailable/)).not.toBeInTheDocument();
 expect(screen.getByPlaceholderText('Your answer…')).toBeDisabled();
 fireEvent.click(retry);await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(2));
});
it.each(['accepted','unknown','in_flight','cancelled','payload_changed','readonly','foreign_read','failed_read','missing_projection'] as const)('FEUI-3 never replays a retained response after %s readback',async reason=>{
 const Detail=await component();const payload={questions:[{question:'Immutable guard?',options:[]}]};
 const request={request_id:'guarded',request_kind:'user-input',payload};
 const exact={decision:'allow' as const,updated_input:{...payload,answers:{'Immutable guard?':'Original retained answer'}}};
 let attempted=false;
 const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]})});
 native.invoke.mockImplementation(cmd=>{
  if(cmd==='delegation_respond'){attempted=true;return Promise.reject(new Error('Unconfirmed answer'));}
  if(cmd==='delegation_read') {
   if(attempted && reason==='failed_read')return Promise.reject(new Error('Read still unavailable'));
   const outcome=['accepted','unknown','in_flight'].includes(reason)?reason:'before_send';
   return Promise.resolve({task:{...task,parent_thread_id:attempted && reason==='foreign_read'?'foreign':'thread-1',cancel_requested:attempted && reason==='cancelled',responses:attempted && reason!=='missing_projection'?[{request_id:'guarded',outcome,decision:exact,eligible:!['accepted','unknown','in_flight','cancelled','payload_changed'].includes(reason),reason:reason==='payload_changed'?'payload_changed':null}]:[]},events:[],next_cursor:0,has_more:false});
  }
  return Promise.resolve([task]);
 });
 renderHook(()=>useDelegationTasks('pane','thread-1'));
 const props={task,paneId:'pane',writable:true,onBack:vi.fn()};const view=render(<Detail {...props}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Original retained answer'}});fireEvent.click(screen.getByRole('button',{name:'Send'}));
 await screen.findByText(/Approval response unconfirmed: Unconfirmed answer/);
 if(reason==='readonly')view.rerender(<Detail {...props} writable={false}/>);
 fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 const retry=screen.queryByRole('button',{name:'Retry exact response guarded'});if(retry){expect(retry).toBeDisabled();fireEvent.click(retry);}
 expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();expect(screen.getByRole('button',{name:'Deny request guarded'})).toBeDisabled();
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
});
it('consent client never recaptures a changed payload for a retained displayed Allow callback',async()=>{
 const Detail=await component();const request={request_id:'displayed',request_kind:'permission',payload:{command:'Original displayed command'}};
 const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:'displayed',outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
 let reading=task;
 native.invoke.mockImplementation(cmd=>Promise.resolve(cmd==='delegation_read'?{task:reading,events:[],next_cursor:0,has_more:false}:cmd==='delegation_respond'?reading:[reading]));
 renderHook(()=>useDelegationTasks('pane','thread-1'));render(<Detail task={task} paneId="pane" writable onBack={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Allow request displayed'})).toBeEnabled());
 const button=screen.getByRole('button',{name:'Allow request displayed'});
 // Keep the actual rendered callback, not a replacement mutation function.
 const propsKey=Object.keys(button).find(key=>key.startsWith('__reactProps'))!;
 const displayed=(button as unknown as Record<string,{onClick:()=>void}>)[propsKey].onClick;
 reading={...task,remote:remoteFixture({pending_requests:[{...request,payload:{command:'Replacement command'}}]})};
 fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 await act(async()=>displayed());
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(0);
});
it.each(['unmounted', 'mounted-owner-replaced'] as const)('closure retry fences every obsolete rendered mutation callback for %s',async schedule=>{
 const Detail=await component();
 for(const kind of ['allow','deny','question','stop','deliver'] as const){
  const request={request_id:'same-request',request_kind:kind==='question'?'user-input':'permission',payload:kind==='question'?{questions:[{question:'Exact question?',options:[]}]}:{command:'Same displayed command'}};
  const task=taskFixture({status:kind==='deliver'?'completed':'awaiting_approval',wake_state:'held',delivery:{outcome:'before_send',eligible:true,reason:null},remote:remoteFixture({status:kind==='deliver'?'completed':'awaiting_approval',pending_requests:kind==='deliver'||kind==='stop'?[]:[request]}),responses:[{request_id:request.request_id,outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
  const command=kind==='stop'?'delegation_cancel':kind==='deliver'?'delegation_deliver':'delegation_respond';
  native.invoke.mockImplementation(cmd=>Promise.resolve(cmd==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:cmd===command?task:[task]));
  let owner=renderHook(()=>useDelegationTasks('pane','thread-1'));
  const props={task,paneId:'pane',writable:true,onBack:vi.fn()};
  let view=render(<Detail {...props}/>);
  const name=kind==='question'?'Send':kind==='stop'?'Stop task':kind==='deliver'?'Deliver to parent':`${kind==='allow'?'Allow':'Deny'} request same-request`;
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  if(kind==='question')fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Exact original answer'}});
  await waitFor(()=>expect(screen.getByRole('button',{name})).toBeEnabled());
  const node=kind==='question'?view.container.querySelector('form')!:screen.getByRole('button',{name});
  const propsKey=Object.keys(node).find(key=>key.startsWith('__reactProps'))!;
  const rendered=(node as unknown as Record<string,{onClick:()=>void;onSubmit:(e:{preventDefault:()=>void})=>void}>)[propsKey];
  const obsolete=kind==='question'?()=>rendered.onSubmit({preventDefault:vi.fn()}):rendered.onClick;
  const originalOwner=delegationResponseReadBoundary('pane','thread-1',task.id)!.owner;
  if(schedule==='unmounted')view.unmount();
  owner.unmount();
  owner=renderHook(()=>useDelegationTasks('pane','thread-1'));
  if(schedule==='unmounted')view=render(<Detail {...props}/>);else view.rerender(<Detail {...props}/>);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  // A new owner receives a fresh question draft as well as fresh callbacks.
  if(kind==='question')fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'Exact original answer'}});
  const before=delegationResponseReadBoundary('pane','thread-1',task.id)!;
  expect(before.owner).not.toBe(originalOwner);
  native.invoke.mockClear();
  await act(async()=>obsolete());
  expect(native.invoke.mock.calls, `${kind}: obsolete callback invoked IPC`).toHaveLength(0);
  expect(delegationResponseReadBoundary('pane','thread-1',task.id)).toEqual(before);
  expect(retainedDelegationResponse('pane','thread-1',task.id,request.request_id)).toBeUndefined();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  // A routine render preserves current owner authority; new callbacks work.
  view.rerender(<Detail {...props}/>);
  await waitFor(()=>expect(screen.getByRole('button',{name})).toBeEnabled());
  fireEvent.click(screen.getByRole('button',{name}));
  await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd===command)).toHaveLength(1));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  view.unmount();owner.unmount();native.invoke.mockClear();
 }
});
it.each(['resolve','reject','resolve-after-error','reject-after-error'] as const)('owner lifecycle recovers mounted Stop before old %s and preserves newer state',async completion=>{
 const Detail=await component();const task=taskFixture({remote:remoteFixture()});
 const stops:{resolve:(task:LocalTask)=>void;reject:(error:Error)=>void}[]=[];
 const page:TaskRead={task,events:[{sequence:7,event:{type:'runtime_warning',thread_id:'child-1',message:'Current owner history',original_payload:null}}],next_cursor:7,has_more:false};
 native.invoke.mockImplementation(cmd=>cmd==='delegation_cancel'?new Promise<LocalTask>((resolve,reject)=>stops.push({resolve,reject})):Promise.resolve(cmd==='delegation_read'?page:[task]));
 let owner=renderHook(()=>useDelegationTasks('pane','thread-1'));
 const props={task,paneId:'pane',writable:true,onBack:vi.fn()};const view=render(<Detail {...props}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Stop task'})).toBeEnabled());
 fireEvent.click(screen.getByRole('button',{name:'Stop task'}));expect(stops).toHaveLength(1);
 const oldOwner=delegationResponseReadBoundary('pane','thread-1',task.id)!.owner;
 owner.unmount();owner=renderHook(()=>useDelegationTasks('pane','thread-1'));view.rerender(<Detail {...props}/>);
 expect(delegationResponseReadBoundary('pane','thread-1',task.id)!.owner).not.toBe(oldOwner);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 const before=native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_read').length;
 fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));
 await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_read')).toHaveLength(before+1));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Stop task'})).toBeEnabled());
 fireEvent.click(screen.getByRole('button',{name:'Stop task'}));expect(stops).toHaveLength(2);
 const boundary=delegationResponseReadBoundary('pane','thread-1',task.id);
 if(completion.endsWith('after-error')){await act(async()=>stops[1].reject(new Error('Current owner rejection')));await screen.findByText(/Stop unconfirmed: Current owner rejection/);}
 await act(async()=>completion.startsWith('resolve')?stops[0].resolve({...task,cancel_requested:true}):stops[0].reject(new Error('Old owner rejection')));
 if(completion.endsWith('after-error')){expect(screen.getByRole('alert')).toHaveTextContent('Current owner rejection');expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled();}
 else {expect(screen.getByRole('button',{name:'Stop task'})).toBeDisabled();expect(screen.getByRole('button',{name:'Refresh task'})).toBeDisabled();expect(screen.queryByRole('alert')).not.toBeInTheDocument();}
 expect(screen.getByText('Current owner history')).toBeInTheDocument();
 expect(delegationResponseReadBoundary('pane','thread-1',task.id)).toEqual(boundary);
 if(!completion.endsWith('after-error'))await act(async()=>stops[1].reject(new Error('Current owner rejection')));
 await screen.findByText(/Stop unconfirmed: Current owner rejection/);expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled();
 view.unmount();owner.unmount();
});
it.each(['resolve','reject','resolve-after-error','reject-after-error'] as const)('owner lifecycle isolates deferred old read %s from replacement loading and errors',async completion=>{
 const Detail=await component();const task=taskFixture({remote:remoteFixture()});
 const reads:{resolve:(read:TaskRead)=>void;reject:(error:Error)=>void}[]=[];
 native.invoke.mockImplementation(cmd=>cmd==='delegation_read'?new Promise<TaskRead>((resolve,reject)=>reads.push({resolve,reject})):Promise.resolve([task]));
 let owner=renderHook(()=>useDelegationTasks('pane','thread-1'));const props={task,paneId:'pane',writable:true,onBack:vi.fn()};const view=render(<Detail {...props}/>);
 await waitFor(()=>expect(reads).toHaveLength(1));owner.unmount();owner=renderHook(()=>useDelegationTasks('pane','thread-1'));view.rerender(<Detail {...props}/>);
 await waitFor(()=>expect(reads).toHaveLength(2));
 const oldPage:TaskRead={task:{...task,cancel_requested:true},events:[{sequence:9,event:{type:'runtime_warning',thread_id:'child-1',message:'Obsolete history',original_payload:null}}],next_cursor:9,has_more:false};
 if(completion.endsWith('after-error'))await act(async()=>reads[1].reject(new Error('Current read error')));
 await act(async()=>completion.startsWith('resolve')?reads[0].resolve(oldPage):reads[0].reject(new Error('Obsolete read error')));
 if(completion.endsWith('after-error'))expect(screen.getByRole('alert')).toHaveTextContent('Current read error');
 else {expect(screen.getByRole('button',{name:'Refresh task'})).toBeDisabled();expect(screen.getByText('Reading task history…')).toBeInTheDocument();expect(screen.queryByRole('alert')).not.toBeInTheDocument();}
 expect(screen.queryByText('Obsolete history')).not.toBeInTheDocument();
 if(!completion.endsWith('after-error'))await act(async()=>reads[1].reject(new Error('Current read error')));
 await screen.findByText(/Could not read task history: Current read error/);
 expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled();fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));await waitFor(()=>expect(reads).toHaveLength(3));
 await act(async()=>reads[2].resolve({task,events:[],next_cursor:0,has_more:false}));await waitFor(()=>expect(screen.getByRole('button',{name:'Stop task'})).toBeEnabled());
 view.unmount();owner.unmount();
});
it.each(['question','allow','deny'] as const)('owner lifecycle recovers pending %s only from fresh canonical native outcomes',async kind=>{
 const Detail=await component();
 for(const outcome of ['not_attempted','before_send','accepted','unknown','in_flight'] as const){
  const payload=kind==='question'?{questions:[{question:'Owner answer?',options:[]}]}:{command:'Exact original command'};
  const request={request_id:'owner-request',request_kind:kind==='question'?'user-input':'permission',payload};
  const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:request.request_id,outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
  let current=task;let defer=false;let finishRead!:(read:TaskRead)=>void;
  const responses:{resolve:(task:LocalTask)=>void;reject:(error:Error)=>void}[]=[];
  native.invoke.mockImplementation(cmd=>cmd==='delegation_respond'?new Promise<LocalTask>((resolve,reject)=>responses.push({resolve,reject})):cmd==='delegation_read'?defer?new Promise<TaskRead>(resolve=>{finishRead=resolve;}):Promise.resolve({task:current,events:[],next_cursor:0,has_more:false}):Promise.resolve([current]));
  let owner=renderHook(()=>useDelegationTasks('pane','thread-1'));const props={task,paneId:'pane',writable:true,onBack:vi.fn()};const view=render(<Detail {...props}/>);
  const name=kind==='question'?'Send':`${kind==='allow'?'Allow':'Deny'} request owner-request`;
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  if(kind==='question')fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'First durable answer'}});
  fireEvent.click(screen.getByRole('button',{name}));expect(responses).toHaveLength(1);
  const original=native.invoke.mock.calls.find(([cmd])=>cmd==='delegation_respond')![1];
  current={...task,responses:[{request_id:request.request_id,outcome,decision:outcome==='not_attempted'?null:original.decision,eligible:outcome==='not_attempted'||outcome==='before_send',reason:null}]};
  defer=true;owner.unmount();owner=renderHook(()=>useDelegationTasks('pane','thread-1'));view.rerender(<Detail {...props}/>);
  await waitFor(()=>expect(finishRead).toBeTypeOf('function'));
  expect(screen.getByRole('button',{name:name==='Send'?'Send':'Deny request owner-request'})).toBeDisabled();
  await act(async()=>finishRead({task:current,events:[],next_cursor:0,has_more:false}));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  expect(retainedDelegationResponse('pane','thread-1',task.id,request.request_id)).toBeUndefined();
  if(outcome==='not_attempted'){
   if(kind==='question')fireEvent.change(screen.getByPlaceholderText('Your answer…'),{target:{value:'New answer after native NotAttempted'}});
   await waitFor(()=>expect(screen.getByRole('button',{name})).toBeEnabled());fireEvent.click(screen.getByRole('button',{name}));
  }else if(outcome==='before_send'){
   if(kind==='question')expect(screen.getByPlaceholderText('Your answer…')).toBeDisabled();
   const retry=screen.getByRole('button',{name:'Retry exact response owner-request'});expect(retry).toBeEnabled();fireEvent.click(retry);
   expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')[1][1]).toEqual(original);
  }else{
   expect(screen.queryByRole('button',{name:'Retry exact response owner-request'})).not.toBeInTheDocument();expect(screen.getByRole('button',{name:'Deny request owner-request'})).toBeDisabled();
   if(kind==='question'){expect(screen.getByPlaceholderText('Your answer…')).toBeDisabled();expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();}
  }
  const boundary=delegationResponseReadBoundary('pane','thread-1',task.id);const count=responses.length;
  await act(async()=>responses[0].reject(new Error('Obsolete response rejection')));
  expect(delegationResponseReadBoundary('pane','thread-1',task.id)).toEqual(boundary);expect(screen.queryByRole('alert')).not.toBeInTheDocument();expect(responses).toHaveLength(count);
  if(count===2){expect(screen.getByRole('button',{name:'Refresh task'})).toBeDisabled();await act(async()=>responses[1].reject(new Error('Current response rejection')));await screen.findByText(/Current response rejection/);}
  else expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled();
  view.unmount();owner.unmount();native.invoke.mockClear();
 }
});
it.each(['read-before-reject' ,'read-after-reject','reopen-pending','readonly-pending'] as const)('consent settlement rejects event/pagination qualification from %s',async schedule=>{
 const Detail=await component();
 const request={request_id:'boundary',request_kind:'permission',payload:{command:'Exact original'}};
 const original=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:'boundary',outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
 let listed=original;let rejectRespond!:(error:Error)=>void;let lateRead!:(read:TaskRead)=>void;let deferRead=false;
 let invalidate!:(event:{payload:{parent_thread_id:string}})=>void;
 native.listen.mockImplementation((_event,cb)=>{invalidate=cb;return Promise.resolve(vi.fn());});
 const page=(task:LocalTask,cursor=1):TaskRead=>({task,events:[{sequence:cursor,event:{type:'runtime_warning',thread_id:'child-1',message:`Record ${cursor}`,original_payload:null}}],next_cursor:cursor,has_more:true});
 native.invoke.mockImplementation((cmd,args)=>{
  if(cmd==='delegation_respond')return new Promise((_resolve,reject)=>{rejectRespond=reject;});
  if(cmd==='delegation_read')return deferRead?new Promise<TaskRead>(resolve=>{lateRead=resolve;}):Promise.resolve(page(listed,(args.cursor??0)+1));
  return Promise.resolve([listed]);
 });
 function Owner({open=true,writable=true}:{open?:boolean;writable?:boolean}){const state=useDelegationTasks('pane','thread-1');return open?<Detail task={state.tasks[0]??listed} paneId="pane" writable={writable} onBack={vi.fn()}/>:null;}
 const view=render(<Owner/>);await waitFor(()=>expect(screen.getByRole('button',{name:'Allow request boundary'})).toBeEnabled());
 fireEvent.click(screen.getByRole('button',{name:'Allow request boundary'}));
 await waitFor(()=>expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1));
 // Force an actual native-event -> shared store -> revision detail read while
 // the mutation is unresolved. Disabling pagination alone cannot close this.
 deferRead=true;listed={...original,updated_at:'event-read-during-response'};
 await act(async()=>invalidate({payload:{parent_thread_id:'thread-1'}}));
 await waitFor(()=>expect(lateRead).toBeTypeOf('function'));
 if(schedule==='reopen-pending'){view.rerender(<Owner open={false}/>);view.rerender(<Owner/>);await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeDisabled());}
 if(schedule==='readonly-pending')view.rerender(<Owner writable={false}/>);
 if(schedule==='read-before-reject')await act(async()=>lateRead(page(listed,2)));
 await act(async()=>rejectRespond(new Error('Ambiguous response rejection')));
 if(schedule!=='read-before-reject')await act(async()=>lateRead(page(listed,2)));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 expect(screen.queryByRole('button',{name:'Retry exact response boundary'})).not.toBeInTheDocument();
 expect(screen.getByRole('button',{name:'Deny request boundary'})).toBeDisabled();
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
 // Only an explicit fresh read begun after rejection can establish retry.
 deferRead=false;if(schedule==='readonly-pending')view.rerender(<Owner/>);
 fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));
 const retry=await screen.findByRole('button',{name:'Retry exact response boundary'});await waitFor(()=>expect(retry).toBeEnabled());
 fireEvent.click(retry);expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(2);
 await act(async()=>rejectRespond(new Error('Second attempt settled')));
});
it.each(['eligible-success','accepted-success','failed-confirm'] as const)('consent settlement preserves its own post-success read for %s',async outcome=>{
 const Detail=await component();const request={request_id:'success-boundary',request_kind:'permission',payload:{command:'Original'}};
 const exact={decision:'deny' as const,message:'Denied by user in parent conversation.'};
 const original=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:request.request_id,outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
 let resolveRespond!:(task:LocalTask)=>void;let finishRead!:(read:TaskRead)=>void;let rejectRead!:(error:Error)=>void;let confirm=false;
 native.invoke.mockImplementation(cmd=>cmd==='delegation_respond'?new Promise<LocalTask>(resolve=>{resolveRespond=resolve;}):cmd==='delegation_read'?confirm?new Promise<TaskRead>((resolve,reject)=>{finishRead=resolve;rejectRead=reject;}):Promise.resolve({task:original,events:[],next_cursor:0,has_more:false}):Promise.resolve([original]));
 renderHook(()=>useDelegationTasks('pane','thread-1'));render(<Detail task={original} paneId="pane" writable onBack={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:`Deny request ${request.request_id}`})).toBeEnabled());fireEvent.click(screen.getByRole('button',{name:`Deny request ${request.request_id}`}));
 const settled={...original,responses:[{request_id:request.request_id,outcome:outcome==='accepted-success'?'accepted' as const:'before_send' as const,decision:exact,eligible:outcome!=='accepted-success',reason:null}]};
 confirm=true;await act(async()=>resolveRespond(settled));await waitFor(()=>expect(finishRead).toBeTypeOf('function'));
 expect(screen.queryByRole('button',{name:`Retry exact response ${request.request_id}`})).not.toBeInTheDocument();
 if(outcome==='failed-confirm')await act(async()=>rejectRead(new Error('Readback failed')));else await act(async()=>finishRead({task:settled,events:[],next_cursor:0,has_more:false}));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
 const retry=screen.queryByRole('button',{name:`Retry exact response ${request.request_id}`});
 if(outcome==='eligible-success')expect(retry).toBeEnabled();else expect(retry).not.toBeInTheDocument();
 expect(screen.getByRole('button',{name:`Deny request ${request.request_id}`})).toBeDisabled();
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
});
it('consent settlement cannot qualify an overlapping pagination read after rejected Respond',async()=>{
 const Detail=await component();const request={request_id:'pagination',request_kind:'permission',payload:{command:'Original'}};
 const task=taskFixture({status:'awaiting_approval',remote:remoteFixture({pending_requests:[request]}),responses:[{request_id:'pagination',outcome:'not_attempted',decision:null,eligible:true,reason:null}]});
 let rejectRespond!:(error:Error)=>void;let resolvePage!:(read:TaskRead)=>void;
 native.invoke.mockImplementation((cmd,args)=>cmd==='delegation_respond'?new Promise((_r,reject)=>{rejectRespond=reject;}):cmd==='delegation_read' && args.cursor?new Promise<TaskRead>(resolve=>{resolvePage=resolve;}):Promise.resolve(cmd==='delegation_read'?{task,events:[{sequence:1,event:{type:'runtime_warning',thread_id:'child-1',message:'First event',original_payload:null}}],next_cursor:1,has_more:true}:[task]));
 renderHook(()=>useDelegationTasks('pane','thread-1'));render(<Detail task={task} paneId="pane" writable onBack={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Allow request pagination'})).toBeEnabled());fireEvent.click(screen.getByRole('button',{name:'Allow request pagination'}));
 fireEvent.click(screen.getByRole('button',{name:'Load more events'}));
 if(resolvePage)await act(async()=>resolvePage({task,events:[],next_cursor:2,has_more:false}));
 await act(async()=>rejectRespond(new Error('Pagination overlapped rejection')));
 await screen.findByText(/Approval response unconfirmed: Pagination overlapped rejection/);
 expect(screen.queryByRole('button',{name:'Retry exact response pagination'})).not.toBeInTheDocument();
 expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
});
it.skipIf(!process.env.CODEMUX_UI_RECOVERY_CAPTURE)('FEUI-3 replays actual native decision DTOs through IPC, deferred readback and detail close/reopen without callback replay',async()=>{
 const Detail=await component();
 const receipts=JSON.parse(readFileSync(process.env.CODEMUX_UI_RECOVERY_CAPTURE!,'utf8')) as {before_send:boolean;pane:string;thread:string;first:LocalTask;first_read:TaskRead;after_read:TaskRead;args:Record<string,unknown>}[];
 expect(receipts).toHaveLength(2);
 for(const receipt of receipts) {
  let readingAfter=false;let defer=false;let resolveRead!:(read:TaskRead)=>void;
  native.invoke.mockImplementation((cmd,args)=>{
   if(cmd==='delegation_respond'){expect(args).toEqual({...receipt.args,originalRequest:receipt.first.remote!.pending_requests[0],decision:receipt.first.responses[0].decision});readingAfter=true;defer=true;return Promise.resolve(receipt.after_read.task);}
   if(cmd==='delegation_read'){if(defer)return new Promise<TaskRead>(resolve=>{resolveRead=resolve;});return Promise.resolve(readingAfter?receipt.after_read:receipt.first_read);}
   return Promise.resolve([receipt.first]);
  });
  const owner=renderHook(()=>useDelegationTasks(receipt.pane,receipt.thread));
  let view=render(<Detail task={receipt.first} paneId={receipt.pane} writable onBack={vi.fn()}/>);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  expect(screen.getByRole('button',{name:'Send'})).toBeDisabled();
  if(receipt.before_send) {
   fireEvent.click(screen.getByRole('button',{name:'Retry exact response recovery'}));
   await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeDisabled());
   view.unmount();defer=false;
   view=render(<Detail task={receipt.after_read.task} paneId={receipt.pane} writable onBack={vi.fn()}/>);
   await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
   await act(async()=>resolveRead(receipt.first_read));
   expect(screen.queryByRole('button',{name:'Retry exact response recovery'})).not.toBeInTheDocument();
   expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(1);
  } else {
   expect(screen.queryByRole('button',{name:'Retry exact response recovery'})).not.toBeInTheDocument();
   fireEvent.click(screen.getByRole('button',{name:'Refresh task'}));
   await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
   expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegation_respond')).toHaveLength(0);
  }
  view.unmount();owner.unmount();native.invoke.mockClear();
 }
});
it('delivers a held result explicitly once and confirms parent delivery instead of blindly resending', async () => {
  const Detail = await component();
  let task = taskFixture({ status: 'completed', wake_state: 'held', delivery:{outcome:'before_send',eligible:true,reason:null}, remote: remoteFixture({ status: 'completed', result: 'Actual final report' }) });
  let resolveDelivery!: (task: LocalTask) => void;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_deliver') return new Promise<LocalTask>(resolve => { resolveDelivery = resolve; });
    if (command === 'delegation_read') return Promise.resolve({ task, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([task]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Deliver to parent' })).toBeEnabled());
  const deliver = screen.getByRole('button', { name: 'Deliver to parent' });
  fireEvent.click(deliver);
  fireEvent.click(deliver);
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_deliver')).toHaveLength(1);
  expect(screen.queryByText('Delivered to parent')).not.toBeInTheDocument();
  task = { ...task, wake_state: 'delivered' };
  await act(async () => { resolveDelivery(task); });
  await screen.findByText('Delivered to parent');
  expect(native.invoke).toHaveBeenCalledWith('delegation_deliver', { paneId: 'pane', taskId: 'task-1' });
});

it.each(['accepted', 'unknown', 'cancelled', 'no_result'] as const)('never offers resend from held presentation when native authority says %s', async reason => {
  const Detail = await component();
  const task: LocalTask = { ...taskFixture({status:'completed',wake_state:'held',remote:remoteFixture({status:'completed'})}), delivery: {outcome:reason==='accepted'?'accepted':reason==='unknown'?'unknown':'before_send',eligible:false,reason} };
  native.invoke.mockImplementation(command => Promise.resolve(command==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:[task]));
  renderHook(() => useDelegationTasks('pane','thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button',{name:'Refresh task'})).toBeEnabled());
  expect(screen.queryByRole('button',{name:'Deliver to parent'})).not.toBeInTheDocument();
  expect(screen.queryByText(/Deliver this recorded result explicitly/)).not.toBeInTheDocument();
});
it('offers native-eligible suppressed-before-send results without inferring readiness from an error', async () => {
  const Detail = await component();
  const task: LocalTask = { ...taskFixture({status:'completed',wake_state:'suppressed',wake_error:'accepted in a diagnostic string is not authority',remote:remoteFixture({status:'completed'})}),delivery:{outcome:'before_send',eligible:true,reason:null} };
  native.invoke.mockImplementation(command=>Promise.resolve(command==='delegation_read'?{task,events:[],next_cursor:0,has_more:false}:[task]));
  renderHook(()=>useDelegationTasks('pane','thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Deliver to parent'})).toBeEnabled());
});
it('does not act from readonly or foreign-thread presentation and drops post-unmount action readback', async () => {
  const Detail = await component();
  const task = taskFixture({ remote: remoteFixture() });
  let resolveStop!: (task: LocalTask) => void;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') return new Promise<LocalTask>(resolve => { resolveStop = resolve; });
    if (command === 'delegation_read') return Promise.resolve({ task, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([task]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const props = { task, paneId: 'pane', writable: false, onBack: vi.fn() };
  const view = render(<Detail {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.getByRole('button', { name: 'Stop task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  view.rerender(<Detail {...props} writable task={taskFixture({ parent_thread_id: 'foreign-thread', remote: remoteFixture() })} />);
  expect(screen.getByRole('button', { name: 'Stop task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toHaveLength(0);
  view.rerender(<Detail {...props} writable />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  const readCount = native.invoke.mock.calls.filter(([command]) => command === 'delegation_read').length;
  view.unmount();
  await act(async () => { resolveStop(taskFixture({ status: 'stopping', cancel_requested: true })); });
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_read')).toHaveLength(readCount);
});

it.each([
  { completion: 'resolve', newerFirst: false },
  { completion: 'reject', newerFirst: false },
  { completion: 'resolve', newerFirst: true },
  { completion: 'reject', newerFirst: true },
])('NGI-1 reconciles a superseded Stop confirmation through the actual invalidation/list/revision schedule after old read $completion (newer first: $newerFirst)', async ({ completion, newerFirst }) => {
  const Detail = await component();
  const original = taskFixture({ remote: remoteFixture() });
  let listed = original;
  let stopping = false;
  let invalidate!: (event: { payload: { parent_thread_id: string } }) => void;
  const reads: { resolve: (read: TaskRead) => void; reject: (error: Error) => void }[] = [];
  native.listen.mockImplementation((_event, callback) => { invalidate = callback; return Promise.resolve(vi.fn()); });
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') { stopping = true; return Promise.resolve({ ...original, cancel_requested: true }); }
    if (command === 'delegation_read') return stopping ? new Promise<TaskRead>((resolve, reject) => reads.push({ resolve, reject })) : Promise.resolve({ task: original, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([listed]);
  });
  function Owner() {
    const state = useDelegationTasks('pane', 'thread-1');
    return <Detail task={state.tasks[0] ?? original} paneId="pane" writable onBack={vi.fn()} />;
  }
  render(<Owner />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  await waitFor(() => expect(reads).toHaveLength(1));
  listed = { ...original, updated_at: 'stop-invalidated', status: 'stopping', cancel_requested: true, remote: remoteFixture({ status: 'stopping', cancel_requested: true }) };
  await act(async () => invalidate({ payload: { parent_thread_id: 'thread-1' } }));
  await waitFor(() => expect(reads).toHaveLength(2));
  if (newerFirst) await act(async () => reads[1].resolve({ task: listed, events: [], next_cursor: 0, has_more: false }));
  await act(async () => completion === 'resolve' ? reads[0].resolve({ task: original, events: [], next_cursor: 0, has_more: false }) : reads[0].reject(new Error('Obsolete confirmation transport')));
  expect(screen.queryByText(/Stop unconfirmed/)).not.toBeInTheDocument();
  expect(screen.getByText('Stopping')).toBeInTheDocument();
  expect(screen.getByText('Stop requested; awaiting remote confirmation.')).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  if (!newerFirst) await act(async () => reads[1].resolve({ task: listed, events: [], next_cursor: 0, has_more: false }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  listed = { ...listed, updated_at: 'remote-confirmed', status: 'cancelled', remote: remoteFixture({ status: 'cancelled', cancel_requested: true }) };
  await act(async () => invalidate({ payload: { parent_thread_id: 'thread-1' } }));
  await waitFor(() => expect(reads).toHaveLength(3));
  await act(async () => reads[2].resolve({ task: listed, events: [], next_cursor: 0, has_more: false }));
  await screen.findByText('Stopped');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: original.id }]]);
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_list')).toHaveLength(3);
  expect(native.listen).toHaveBeenCalledWith('delegation-changed', expect.any(Function));
});

it('NGI-1 Refresh requires a post-settlement read for the same Stop mutation', async () => {
  const Detail = await component();
  const original = taskFixture({ remote: remoteFixture() });
  let listed = original;
  let deferRead = false;
  let invalidate!: (event: { payload: { parent_thread_id: string } }) => void;
  let rejectStop!: (error: Error) => void;
  let finishRead!: (read: TaskRead) => void;
  native.listen.mockImplementation((_event, callback) => { invalidate = callback; return Promise.resolve(vi.fn()); });
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') return new Promise((_resolve, reject) => { rejectStop = reject; });
    if (command === 'delegation_read') return deferRead ? new Promise<TaskRead>(resolve => { finishRead = resolve; }) : Promise.resolve({ task: listed, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([listed]);
  });
  function Owner() {
    const state = useDelegationTasks('pane', 'thread-1');
    return <Detail task={state.tasks[0] ?? original} paneId="pane" writable onBack={vi.fn()} />;
  }
  render(<Owner />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  deferRead = true;
  listed = { ...original, updated_at: 'during-stop', status: 'stopping', cancel_requested: true };
  await act(async () => invalidate({ payload: { parent_thread_id: 'thread-1' } }));
  await waitFor(() => expect(finishRead).toBeTypeOf('function'));
  await act(async () => rejectStop(new Error('Stop acknowledgement lost')));
  await screen.findByText(/Stop unconfirmed: Stop acknowledgement lost/);
  await act(async () => finishRead({ task: listed, events: [], next_cursor: 0, has_more: false }));
  expect(screen.getByRole('alert')).toHaveTextContent('Stop acknowledgement lost');
  deferRead = false;
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  expect(screen.getByText('Stopping')).toBeInTheDocument();
  expect(screen.getByText('Stop requested; awaiting remote confirmation.')).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toHaveLength(1);
});

it.each(['unchanged', 'foreign', 'readonly', 'failed'] as const)('NGI-1 preserves genuine Stop failure across %s Refresh until an exact writable read confirms cancellation intent', async qualification => {
  const Detail = await component();
  const original = taskFixture({ remote: remoteFixture() });
  let reading = original;
  let failRead = false;
  native.invoke.mockImplementation(command => command === 'delegation_cancel' ? Promise.reject(new Error('Cancel transport refused')) : command === 'delegation_read' ? failRead ? Promise.reject(new Error('Read transport refused')) : Promise.resolve({ task: reading, events: [], next_cursor: 0, has_more: false }) : Promise.resolve([original]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const props = { task: original, paneId: 'pane', writable: true, onBack: vi.fn() };
  const view = render(<Detail {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  await screen.findByText(/Stop unconfirmed: Cancel transport refused/);
  reading = qualification === 'unchanged' ? original : { ...original, status: 'cancelled', cancel_requested: true, parent_thread_id: qualification === 'foreign' ? 'foreign-thread' : original.parent_thread_id };
  failRead = qualification === 'failed';
  if (qualification === 'readonly') view.rerender(<Detail {...props} writable={false} />);
  else fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.getByText(/Stop unconfirmed: Cancel transport refused/)).toBeInTheDocument();
  failRead = false;
  reading = { ...original, status: 'cancelled', cancel_requested: true, remote: remoteFixture({ status: 'cancelled', error: 'Unrelated execution diagnostic' }) };
  if (qualification === 'readonly') view.rerender(<Detail {...props} />);
  else fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await waitFor(() => expect(screen.queryByText(/Stop unconfirmed/)).not.toBeInTheDocument());
  expect(screen.getByText('Unrelated execution diagnostic')).toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toHaveLength(1);
});

it.each(['approval', 'deliver'] as const)('NGI-1 Stop reconciliation does not clear unrelated %s alerts', async kind => {
  const Detail = await component();
  const request = { request_id: 'unrelated', request_kind: 'permission', payload: { command: 'Exact permission' } };
  const original = taskFixture({ status: kind === 'deliver' ? 'completed' : 'awaiting_approval', wake_state: 'held', delivery: { outcome: 'before_send', eligible: true, reason: null }, remote: remoteFixture({ pending_requests: kind === 'approval' ? [request] : [] }) });
  let reading = original;
  native.invoke.mockImplementation(command => command === 'delegation_respond' || command === 'delegation_deliver' ? Promise.reject(new Error('Unrelated transport failure')) : Promise.resolve(command === 'delegation_read' ? { task: reading, events: [], next_cursor: 0, has_more: false } : [original]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={original} paneId="pane" writable onBack={vi.fn()} />);
  const name = kind === 'approval' ? 'Allow request unrelated' : 'Deliver to parent';
  await waitFor(() => expect(screen.getByRole('button', { name })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name }));
  await screen.findByText(/unconfirmed: Unrelated transport failure/);
  reading = { ...original, cancel_requested: true, status: 'cancelled' };
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh task' })).toBeEnabled());
  expect(screen.getByRole('alert')).toHaveTextContent('Unrelated transport failure');
});

it('shows stopping only as requested until the receiver confirms cancelled and preserves transport failures', async () => {
  const Detail = await component();
  let task = taskFixture({ remote: remoteFixture() });
  let failReads = false;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') {
      task = taskFixture({ status: 'stopping', cancel_requested: true, remote: remoteFixture({ status: 'stopping', cancel_requested: true }) });
      failReads = true;
      return Promise.resolve(task);
    }
    if (command === 'delegation_read') return failReads ? Promise.reject(new Error('SSH unavailable')) : Promise.resolve({ task, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([task]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Detail task={task} paneId="pane" writable onBack={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  await screen.findByText(/Stop unconfirmed: SSH unavailable/);
  expect(screen.getByText('Running')).toBeInTheDocument();
  expect(screen.getByText('Stop requested; awaiting remote confirmation.')).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  failReads = false;
  task = taskFixture({ status: 'cancelled', cancel_requested: true, remote: remoteFixture({ status: 'cancelled', cancel_requested: true }), wake_state: 'suppressed' });
  fireEvent.click(screen.getByRole('button', { name: 'Refresh task' }));
  await screen.findByText('Stopped');
  expect(screen.queryByRole('button', { name: 'Stop task' })).not.toBeInTheDocument();
  expect(screen.queryByText(/Stop unconfirmed/)).not.toBeInTheDocument();
});

it('drops late history from another selected task while keeping event sequence stable', async () => {
  const Detail = await component();
  const first = taskFixture();
  const second = taskFixture({ id: 'task-2', title: 'Second task' });
  let resolveFirst!: (read: TaskRead) => void;
  native.invoke.mockImplementation((command, args) => {
    if (command === 'delegation_read') return args.taskId === 'task-1' ? new Promise<TaskRead>(resolve => { resolveFirst = resolve; }) : Promise.resolve({ task: second, events: [{ sequence: 9, event: { type: 'runtime_warning', thread_id: 'child-2', message: 'Second task record', original_payload: null } }], next_cursor: 9, has_more: false });
    return Promise.resolve([first, second]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const view = render(<Detail task={first} paneId="pane" writable={false} onBack={vi.fn()} />);
  view.rerender(<Detail task={second} paneId="pane" writable={false} onBack={vi.fn()} />);
  await screen.findByText('Second task record');
  await act(async () => { resolveFirst({ task: first, events: [{ sequence: 1, event: { type: 'runtime_warning', thread_id: 'child-1', message: 'Stale task record', original_payload: null } }], next_cursor: 1, has_more: false }); });
  expect(screen.queryByText('Stale task record')).not.toBeInTheDocument();
  expect(screen.getByText('Event #9')).toBeInTheDocument();
});
