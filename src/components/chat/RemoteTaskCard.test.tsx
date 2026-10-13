import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { delegationResponseReadBoundary, upsertDelegationTask, useDelegationTasks } from '@/stores/delegation-store';
import { remoteFixture, taskFixture } from '@/lib/delegation.test-fixtures';
import type { LocalTask } from '@/lib/delegation';
import userEvent from '@testing-library/user-event';
const native = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

/** Synthetic IPC DTOs derived from Journal::cancel/app::project_task and the
 * receiver's terminal projection, not a native-runtime capture. */
function stopSnapshots() {
  const running = taskFixture({ remote: remoteFixture() });
  const stopping = { ...running, updated_at: '2026-10-08T10:00:01Z', status: 'stopping' as const, cancel_requested: true, wake_state: 'suppressed' as const, delivery: { outcome: 'not_attempted' as const, eligible: false, reason: 'cancelled' as const } };
  const cancelled = { ...stopping, updated_at: '2026-10-08T10:00:02Z', status: 'cancelled' as const, remote: remoteFixture({ updated_at: '2026-10-08T10:00:02Z', status: 'cancelled', cancel_requested: true, pending_requests: [], activity: null, result: null, error: null }) };
  return { running, stopping, cancelled };
}

it('F6 preserves a newer cancelled shared-list publication when an older Stop readback resolves last in the same owner', async () => {
  const Card = await component();
  const { running, stopping, cancelled } = stopSnapshots();
  const original = JSON.stringify({ running, stopping, cancelled });
  const cancel = deferred<LocalTask>();
  const read = deferred<import('@/lib/delegation').TaskRead>();
  const terminalList = deferred<LocalTask[]>();
  let invalidate!: (event: { payload: { parent_thread_id: string } }) => void;
  const unlisten = vi.fn();
  let invalidations = 0;
  native.listen.mockImplementation((event, callback) => { expect(event).toBe('delegation-changed'); invalidate = notice => { invalidations++; callback(notice); }; return Promise.resolve(unlisten); });
  let lists = 0;
  native.invoke.mockImplementation((command, args) => {
    if (command === 'delegation_cancel') { expect(args).toEqual({ paneId: 'pane', taskId: running.id }); return cancel.promise; }
    if (command === 'delegation_read') { expect(args).toEqual({ paneId: 'pane', taskId: running.id, cursor: 0 }); return read.promise; }
    expect(command).toBe('delegation_list');
    expect(args).toEqual({ paneId: 'pane' });
    lists++;
    if (lists === 1) return Promise.resolve([running]);
    if (lists === 2) return Promise.resolve([stopping]);
    expect(lists).toBe(3);
    return terminalList.promise;
  });
  let state!: ReturnType<typeof useDelegationTasks>;
  function Owner() {
    state = useDelegationTasks('pane', 'thread-1');
    return state.tasks[0] ? <Card key={state.tasks[0].id} task={state.tasks[0]} paneId="pane" writable onOpen={vi.fn()} /> : null;
  }
  const view = render(<Owner />);
  await waitFor(() => { expect(state.subscriptionHealth).toBe('connected'); expect(state.loading).toBe(false); expect(screen.getByText('Running')).toBeInTheDocument(); });
  const article = screen.getByRole('article');
  const open = screen.getByRole('button', { name: 'View task' });
  const owner = delegationResponseReadBoundary('pane', 'thread-1', running.id)!.owner;
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  const notices = [{ payload: { parent_thread_id: 'thread-1' } }, { payload: { parent_thread_id: 'thread-1' } }];
  await act(async () => invalidate(notices[0]));
  await waitFor(() => { expect(state.tasks[0]).toEqual(stopping); expect(state.loading).toBe(false); });
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(0);
  await act(async () => cancel.resolve(stopping));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(1));
  await act(async () => invalidate(notices[1]));
  expect(state.loading).toBe(true);
  await act(async () => terminalList.resolve([cancelled]));
  await waitFor(() => { expect(state.tasks[0]).toEqual(cancelled); expect(state.loading).toBe(false); expect(screen.getByText('Stopped')).toBeInTheDocument(); });
  expect(screen.getByRole('article')).toBe(article);
  expect(screen.getByRole('button', { name: 'View task' })).toBe(open);
  expect(delegationResponseReadBoundary('pane', 'thread-1', running.id)!.owner).toBe(owner);
  // No additional invalidation or corrective refresh after this old response.
  await act(async () => read.resolve({ task: stopping, events: [], next_cursor: 0, has_more: false }));
  expect(state.tasks[0], 'F6: old Stop read must not replace a fully settled cancelled list').toEqual(cancelled);
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(screen.getByRole('article')).toBe(article);
  expect(screen.queryByRole('button', { name: 'Stop task' })).not.toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['delegation_list', 'delegation_cancel', 'delegation_list', 'delegation_read', 'delegation_list']);
  expect(invalidations).toBe(2);
  expect(delegationResponseReadBoundary('pane', 'thread-1', running.id)!.owner).toBe(owner);
  expect(native.listen).toHaveBeenCalledTimes(1);
  expect(JSON.stringify({ running, stopping, cancelled })).toBe(original);
  view.unmount();
  expect(unlisten).toHaveBeenCalledTimes(1);
});

async function pendingCardStop() {
  const Card = await component();
  const snapshots = stopSnapshots();
  const cancel = deferred<LocalTask>();
  const read = deferred<import('@/lib/delegation').TaskRead>();
  const list = deferred<LocalTask[]>();
  let invalidate!: (event: { payload: { parent_thread_id: string } }) => void;
  const unlisten = vi.fn();
  native.listen.mockImplementation((event, callback) => { expect(event).toBe('delegation-changed'); invalidate = callback; return Promise.resolve(unlisten); });
  let lists = 0;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') return cancel.promise;
    if (command === 'delegation_read') return read.promise;
    expect(command).toBe('delegation_list');
    lists++;
    return lists === 1 ? Promise.resolve([snapshots.running]) : lists === 2 ? list.promise : Promise.resolve([snapshots.cancelled]);
  });
  let state!: ReturnType<typeof useDelegationTasks>;
  function Owner({ writable }: { writable: boolean }) {
    state = useDelegationTasks('pane', 'thread-1');
    return state.tasks[0] ? <Card key={state.tasks[0].id} task={state.tasks[0]} paneId="pane" writable={writable} onOpen={vi.fn()} /> : null;
  }
  const view = render(<Owner writable />);
  await waitFor(() => { expect(state.subscriptionHealth).toBe('connected'); expect(state.loading).toBe(false); expect(state.tasks[0]).toEqual(snapshots.running); });
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  await act(async () => cancel.resolve(snapshots.stopping));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(1));
  return { ...snapshots, view, read, list, unlisten, state: () => state, invalidate: () => invalidate({ payload: { parent_thread_id: 'thread-1' } }), readonly: () => view.rerender(<Owner writable={false} />) };
}

it('F6 accepts read-first Stop confirmation and fences an already pending older list before fresh cancelled reconciliation', async () => {
  const h = await pendingCardStop();
  await act(async () => h.invalidate());
  expect(h.state().loading).toBe(true);
  await act(async () => h.read.resolve({ task: h.stopping, events: [], next_cursor: 0, has_more: false }));
  expect(h.state().tasks[0]).toEqual(h.stopping);
  expect(screen.getByText('Stopping')).toBeInTheDocument();
  await act(async () => h.list.resolve([h.running]));
  await waitFor(() => { expect(h.state().tasks[0]).toEqual(h.cancelled); expect(h.state().loading).toBe(false); });
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(native.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['delegation_list', 'delegation_cancel', 'delegation_read', 'delegation_list', 'delegation_list']);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('F6 does not replace a newer confirmed shared upsert with an older Stop readback', async () => {
  const h = await pendingCardStop();
  act(() => upsertDelegationTask('pane', 'thread-1', h.cancelled));
  expect(h.state().tasks[0]).toEqual(h.cancelled);
  await act(async () => h.read.resolve({ task: h.stopping, events: [], next_cursor: 0, has_more: false }));
  expect(h.state().tasks[0]).toEqual(h.cancelled);
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(native.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['delegation_list', 'delegation_cancel', 'delegation_read']);
});

it.each(['resolve', 'reject'] as const)('F6 discards superseded read $0 without fabricating a Stop error', async completion => {
  const h = await pendingCardStop();
  await act(async () => h.invalidate());
  await act(async () => h.list.resolve([h.cancelled]));
  await waitFor(() => expect(h.state().loading).toBe(false));
  await act(async () => { if (completion === 'resolve') h.read.resolve({ task: h.stopping, events: [], next_cursor: 0, has_more: false }); else h.read.reject(new Error('Obsolete read transport failure')); });
  expect(h.state().tasks[0]).toEqual(h.cancelled);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['delegation_list', 'delegation_cancel', 'delegation_read', 'delegation_list']);
});

it('F6 retains genuine current read failure and permits a fresh exact Stop retry', async () => {
  const h = await pendingCardStop();
  await act(async () => h.read.reject(new Error('Actual read transport failure')));
  expect(screen.getByRole('alert')).toHaveTextContent('Stop unconfirmed: Actual read transport failure. Retry stop to confirm; the remote task may still be running.');
  expect(h.state().tasks[0]).toEqual(h.running);
  const fresh = deferred<import('@/lib/delegation').TaskRead>();
  native.invoke.mockImplementation(command => command === 'delegation_cancel' ? Promise.resolve(h.stopping) : command === 'delegation_read' ? fresh.promise : Promise.resolve([h.running]));
  fireEvent.click(screen.getByRole('button', { name: 'Retry stop' }));
  await waitFor(() => expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(2));
  await act(async () => fresh.resolve({ task: h.cancelled, events: [], next_cursor: 0, has_more: false }));
  expect(h.state().tasks[0]).toEqual(h.cancelled);
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toHaveLength(2);
});

it('F6 retains genuine readback identity errors without publishing foreign tasks', async () => {
  const h = await pendingCardStop();
  await act(async () => h.read.resolve({ task: { ...h.stopping, parent_thread_id: 'foreign' }, events: [], next_cursor: 0, has_more: false }));
  expect(screen.getByRole('alert')).toHaveTextContent('Task readback did not match this conversation.');
  expect(h.state().tasks[0]).toEqual(h.running);
});

it('F6 a failed shared list does not disqualify a current successful Stop read', async () => {
  const h = await pendingCardStop();
  await act(async () => h.invalidate());
  await act(async () => h.list.reject(new Error('Actual list transport failure')));
  await waitFor(() => expect(h.state().loading).toBe(false));
  await act(async () => h.read.resolve({ task: h.stopping, events: [], next_cursor: 0, has_more: false }));
  expect(h.state().tasks[0]).toEqual(h.stopping);
  expect(h.state().error).toBe('Actual list transport failure');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it.each(['readonly', 'disposed'] as const)('F6 denies old read settlement after $0 ownership', async ownership => {
  const h = await pendingCardStop();
  if (ownership === 'readonly') h.readonly();
  else h.view.unmount();
  await act(async () => h.read.resolve({ task: h.stopping, events: [], next_cursor: 0, has_more: false }));
  if (ownership === 'readonly') {
    expect(h.state().tasks[0]).toEqual(h.running);
    expect(screen.getByRole('button', { name: 'Stop task' })).toBeDisabled();
  } else expect(h.unlisten).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.map(([cmd]) => cmd)).toEqual(['delegation_list', 'delegation_cancel', 'delegation_read']);
});

async function component() {
  const modules = import.meta.glob('./RemoteTaskCard.tsx');
  expect(modules['./RemoteTaskCard.tsx'], 'standalone remote receipt must exist').toBeTypeOf('function');
  return (await modules['./RemoteTaskCard.tsx']() as typeof import('./RemoteTaskCard')).RemoteTaskCard;
}
it('P3 cancelled delivery notice card preserves the full diagnostic as muted information without a delivery attempt', async () => {
  const Card = await component();
  const task = taskFixture({ status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' }, remote: remoteFixture({ status: 'cancelled', cancel_requested: true }) });
  const original = JSON.stringify(task);
  native.invoke.mockResolvedValue([task]);
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  const open = vi.fn();
  const view = render(<Card task={task} paneId="pane" writable onOpen={open} />);
  const notice = screen.getByText(`Parent delivery: ${task.wake_error}`, { normalizer: text => text });
  expect(notice.textContent).toBe(`Parent delivery: ${task.wake_error}`);
  expect(notice, 'P3: confirmed stopped no-attempt suppression is informational').toHaveClass('text-muted-foreground');
  expect(notice).not.toHaveClass('text-destructive');
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(screen.getByText('Task was cancelled; parent delivery is unavailable.')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Stop task|Deliver to parent|Retry/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'View task' }));
  expect(open).toHaveBeenCalledWith(task);
  view.rerender(<Card task={task} paneId="pane" writable onOpen={open} />);
  expect(JSON.stringify(task)).toBe(original);
  expect(native.invoke.mock.calls).toEqual([['delegation_list', { paneId: 'pane' }]]);
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
])('P3 cancelled delivery notice card keeps exact diagnostic severity for $qualification', async ({ patch, writable = true, paneId = 'pane', informational = false }) => {
  const Card = await component();
  const task = taskFixture({ status: 'cancelled', cancel_requested: true, wake_state: 'suppressed', wake_error: 'Parent user activity superseded automatic result delivery', delivery: { outcome: 'not_attempted', eligible: false, reason: 'cancelled' }, remote: remoteFixture({ status: 'cancelled', cancel_requested: true }), ...patch });
  const original = JSON.stringify(task);
  native.invoke.mockResolvedValue([task]);
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  const view = render(<Card task={task} paneId={paneId} writable={writable} onOpen={vi.fn()} />);
  const notice = screen.getByText(`Parent delivery: ${task.wake_error}`, { normalizer: text => text });
  expect(notice.textContent).toBe(`Parent delivery: ${task.wake_error}`);
  expect(notice).toHaveClass(informational ? 'text-muted-foreground' : 'text-destructive');
  expect(notice).not.toHaveClass(informational ? 'text-destructive' : 'text-muted-foreground');
  if (task.remote?.error) expect(screen.getByText(task.remote.error).closest('p, [role=alert]')).toHaveClass('text-destructive');
  expect(JSON.stringify(task)).toBe(original);
  expect(view.container.querySelector('a,img')).toBeNull();
  expect(native.invoke.mock.calls.filter(([cmd]) => /cancel|deliver|respond|launch/.test(cmd))).toHaveLength(0);
});

it('P3 cancelled delivery notice card never mutes an actual unconfirmed Stop error', async () => {
  const Card = await component();
  const task = taskFixture({ wake_error: 'Parent user activity superseded automatic result delivery', remote: remoteFixture() });
  native.invoke.mockImplementation(cmd => cmd === 'delegation_cancel' ? Promise.reject(new Error('Actual Stop transport failure')) : Promise.resolve([task]));
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  const view = render(<Card task={task} paneId="pane" writable onOpen={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  const alert = await screen.findByRole('alert');
  const fullError = alert.textContent;
  const stopped = { ...task, status: 'cancelled' as const, cancel_requested: true, wake_state: 'suppressed' as const, delivery: { outcome: 'not_attempted' as const, eligible: false, reason: 'cancelled' as const } };
  view.rerender(<Card task={stopped} paneId="pane" writable onOpen={vi.fn()} />);
  expect(screen.getByRole('alert').textContent).toBe(fullError);
  expect(alert).toHaveClass('text-destructive');
  expect(screen.getByText(`Parent delivery: ${task.wake_error}`)).toHaveClass('text-destructive');
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: task.id }]]);
  expect(native.invoke.mock.calls.filter(([cmd]) => /read|deliver|respond|launch/.test(cmd))).toHaveLength(0);
});

it.each([
  { pending: [{ request_id: 'question', request_kind: 'user-input', payload: { questions: [{ question: 'Choose a route?', options: [] }] } }], summary: 'Waiting for question response' },
  { pending: [{ request_id: 'approval', request_kind: 'command', payload: { command: 'git diff' } }], summary: 'Waiting for approval response' },
  { pending: [{ request_id: 'question', request_kind: 'user-input', payload: { questions: [{ question: 'Choose a route?', options: [] }] } }, { request_id: 'approval', request_kind: 'other', payload: { query: 'Original raw tool input' } }], summary: 'Waiting for question and approval responses' },
])('native question presentation card shows truthful $summary instead of opaque activity without changing the task', async ({ pending: pendingRequests, summary }) => {
  const Card = await component();
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', pending_requests: pendingRequests }) });
  const original = JSON.stringify(task);
  const onOpen = vi.fn();
  const view = render(<Card task={task} paneId="pane" writable={false} onOpen={onOpen} />);
  const waiting = screen.getByText(summary);
  expect(waiting).toHaveAttribute('title', summary);
  expect(screen.queryByText('stream_event message_stop')).not.toBeInTheDocument();
  expect(screen.getByText('Approval needed')).toBeInTheDocument();
  expect(screen.queryByText('Running')).not.toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Stop task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'View task' }));
  expect(onOpen).toHaveBeenCalledWith(task);
  view.rerender(<Card task={task} paneId="pane" writable={false} onOpen={onOpen} />);
  expect(JSON.stringify(task)).toBe(original);
  expect(task.remote?.activity).toBe('stream_event message_stop');
  expect(native.invoke).not.toHaveBeenCalled();
});

it.each([
  { qualification: 'nonwaiting local status', local: 'running' as const, remote: 'awaiting_approval' as const, localCancel: false, remoteCancel: false, requests: true },
  { qualification: 'nonwaiting receiver status', local: 'awaiting_approval' as const, remote: 'running' as const, localCancel: false, remoteCancel: false, requests: true },
  { qualification: 'no pending request', local: 'awaiting_approval' as const, remote: 'awaiting_approval' as const, localCancel: false, remoteCancel: false, requests: false },
  { qualification: 'local cancellation intent', local: 'awaiting_approval' as const, remote: 'awaiting_approval' as const, localCancel: true, remoteCancel: false, requests: true },
  { qualification: 'receiver cancellation intent', local: 'awaiting_approval' as const, remote: 'awaiting_approval' as const, localCancel: false, remoteCancel: true, requests: true },
  { qualification: 'stopping native state', local: 'stopping' as const, remote: 'stopping' as const, localCancel: false, remoteCancel: false, requests: true },
  { qualification: 'terminal native state', local: 'completed' as const, remote: 'completed' as const, localCancel: false, remoteCancel: false, requests: true },
])('native question presentation card keeps genuine activity without inferred waiting from $qualification', async ({ local, remote, localCancel, remoteCancel, requests }) => {
  const Card = await component();
  const task = taskFixture({ status: local, cancel_requested: localCancel, remote: remoteFixture({ status: remote, cancel_requested: remoteCancel, activity: 'stream_event message_stop', pending_requests: requests ? [{ request_id: 'question', request_kind: 'user-input', payload: { questions: [] } }] : [] }) });
  const original = JSON.stringify(task);
  render(<Card task={task} paneId="pane" writable={false} onOpen={vi.fn()} />);
  expect(screen.getByText('stream_event message_stop')).toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(JSON.stringify(task)).toBe(original);
  expect(native.invoke).not.toHaveBeenCalled();
});

it.each([
  { error: 'Actual execution failure', result: 'Actual recorded result', expected: 'Actual execution failure' },
  { error: null, result: 'Actual recorded result', expected: 'Actual recorded result' },
  { error: '', result: 'Actual recorded result', expected: null },
])('native question presentation card preserves error/result priority and independent diagnostics: $error/$result', async ({ error, result, expected }) => {
  const Card = await component();
  const task = taskFixture({ status: 'awaiting_approval', connection_error: 'Actual SSH diagnostic', wake_error: 'Actual delivery diagnostic', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', error, result, pending_requests: [{ request_id: 'question', request_kind: 'user-input', payload: { questions: [] } }] }) });
  const original = JSON.stringify(task);
  render(<Card task={task} paneId="pane" writable={false} onOpen={vi.fn()} />);
  if (expected) expect(screen.getByText(expected)).toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.getByText(/Actual SSH diagnostic/)).toBeInTheDocument();
  expect(screen.getByText('Parent delivery: Actual delivery diagnostic')).toBeInTheDocument();
  expect(screen.getByText('Approval needed')).toBeInTheDocument();
  expect(JSON.stringify(task)).toBe(original);
  expect(native.invoke).not.toHaveBeenCalled();
});

it('native question presentation card keeps awaiting status without inventing waiting content when no receiver snapshot exists', async () => {
  const Card = await component();
  render(<Card task={taskFixture({ status: 'awaiting_approval', remote: null })} paneId="pane" writable={false} onOpen={vi.fn()} />);
  expect(screen.getByText('Approval needed')).toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(native.invoke).not.toHaveBeenCalled();
});

const stopWaitingCases = [
  { kinds: ['user-input'] as const, summary: 'Waiting for question response' },
  { kinds: ['command'] as const, summary: 'Waiting for approval response' },
  { kinds: ['user-input', 'other'] as const, summary: 'Waiting for question and approval responses' },
];

it.each(stopWaitingCases)('native question presentation card restores genuine activity after this card requests Stop without claiming remote cancellation: $summary', async ({ kinds, summary }) => {
  const Card = await component();
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', pending_requests: kinds.map((request_kind, index) => ({ request_id: `pending-${index}`, request_kind, payload: { questions: [] } })) }) });
  const original = JSON.stringify(task);
  let resolveStop!: (task: LocalTask) => void;
  let resolveRead!: (detail: { task: LocalTask; events: []; next_cursor: number; has_more: boolean }) => void;
  const stopping = { ...task, cancel_requested: true, status: 'stopping' as const };
  native.invoke.mockImplementation(cmd => cmd === 'delegation_cancel' ? new Promise<LocalTask>(resolve => { resolveStop = resolve; }) : cmd === 'delegation_read' ? new Promise(resolve => { resolveRead = resolve; }) : Promise.resolve([task]));
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  const onOpen = vi.fn();
  const view = render(<Card task={task} paneId="pane" writable onOpen={onOpen} />);
  expect(screen.getByText(summary)).toBeInTheDocument();
  const stop = screen.getByRole('button', { name: 'Stop task' });
  expect(stop).toBeEnabled();
  fireEvent.click(stop);
  fireEvent.click(stop);
  // Cancel is genuinely unresolved here; no readback has even started.
  expect(stop).toHaveTextContent('Requesting stop…');
  expect(stop).toBeDisabled();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: task.id }]]);
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(0);
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(screen.queryByText('Stop requested; awaiting remote confirmation.')).not.toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/), 'NQP-1: unresolved Cancel must suppress every waiting summary').not.toBeInTheDocument();
  expect(screen.getByText('stream_event message_stop')).toBeInTheDocument();
  expect(screen.getByText('Approval needed')).toBeInTheDocument();
  view.rerender(<Card task={task} paneId="pane" writable={false} onOpen={onOpen} />);
  expect(stop).toHaveTextContent('Requesting stop…');
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  view.rerender(<Card task={task} paneId="pane" writable onOpen={onOpen} />);
  fireEvent.click(stop);
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toHaveLength(1);
  expect(JSON.stringify(task)).toBe(original);
  await act(async () => resolveStop(stopping));
  await waitFor(() => expect(screen.getByText('Stop requested; awaiting remote confirmation.')).toBeInTheDocument());
  expect(stop).toHaveTextContent('Requesting stop…');
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toEqual([['delegation_read', { paneId: 'pane', taskId: task.id, cursor: 0 }]]);
  await act(async () => resolveRead({ task: stopping, events: [], next_cursor: 0, has_more: false }));
  await waitFor(() => expect(stop).toHaveTextContent('Stopping…'));
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.getByText('stream_event message_stop')).toBeInTheDocument();
  expect(screen.getByText('Approval needed')).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: task.id }]]);
  expect(native.invoke).toHaveBeenCalledWith('delegation_read', { paneId: 'pane', taskId: task.id, cursor: 0 });
  expect(owner.result.current.tasks[0]).toEqual(stopping);
  expect(JSON.stringify(task)).toBe(original);
});

it.each(stopWaitingCases)('native question presentation card restores eligible waiting after genuine Cancel rejection and permits one exact retry: $summary', async ({ kinds, summary }) => {
  const Card = await component();
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', pending_requests: kinds.map((request_kind, index) => ({ request_id: `pending-${index}`, request_kind, payload: { questions: [] } })) }) });
  let rejectStop!: (cause: Error) => void;
  let resolveStop!: (task: LocalTask) => void;
  const stopping = { ...task, cancel_requested: true, status: 'stopping' as const };
  native.invoke.mockImplementation(cmd => cmd === 'delegation_cancel' ? new Promise<LocalTask>((resolve, reject) => { resolveStop = resolve; rejectStop = reject; }) : Promise.resolve(cmd === 'delegation_read' ? { task: stopping, events: [], next_cursor: 0, has_more: false } : [task]));
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  render(<Card task={task} paneId="pane" writable onOpen={vi.fn()} />);
  expect(screen.getByText(summary)).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  expect(screen.getByRole('button', { name: 'Stop task' })).toHaveTextContent('Requesting stop…');
  expect(screen.queryByText(/Waiting for question|Waiting for approval/), 'NQP-1: pending rejection must not retain waiting text').not.toBeInTheDocument();
  await act(async () => rejectStop(new Error('Actual Cancel rejection')));
  expect(screen.getByRole('alert')).toHaveTextContent('Stop unconfirmed: Actual Cancel rejection');
  expect(screen.getByText(summary)).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(screen.queryByText('Stop requested; awaiting remote confirmation.')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(0);
  const retry = screen.getByRole('button', { name: 'Retry stop' });
  expect(retry).toBeEnabled();
  fireEvent.click(retry);
  fireEvent.click(retry);
  expect(screen.getByRole('button', { name: 'Stop task' })).toHaveTextContent('Requesting stop…');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toEqual([
    ['delegation_cancel', { paneId: 'pane', taskId: task.id }],
    ['delegation_cancel', { paneId: 'pane', taskId: task.id }],
  ]);
  await act(async () => resolveStop(stopping));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Stop task' })).toHaveTextContent('Stopping…'));
  expect(screen.getByText('Stop requested; awaiting remote confirmation.')).toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toEqual([['delegation_read', { paneId: 'pane', taskId: task.id, cursor: 0 }]]);
});

it.each([
  { error: 'Actual execution failure', result: 'Actual recorded result', expected: 'Actual execution failure' },
  { error: null, result: 'Actual recorded result', expected: 'Actual recorded result' },
  { error: '', result: 'Actual recorded result', expected: null },
])('native question presentation card preserves error/result priority during unresolved Cancel: $error/$result', async ({ error, result, expected }) => {
  const Card = await component();
  const task = taskFixture({ status: 'awaiting_approval', remote: remoteFixture({ status: 'awaiting_approval', activity: 'stream_event message_stop', error, result, pending_requests: [{ request_id: 'question', request_kind: 'user-input', payload: { questions: [] } }] }) });
  let resolveStop!: (task: LocalTask) => void;
  native.invoke.mockImplementation(cmd => cmd === 'delegation_cancel' ? new Promise<LocalTask>(resolve => { resolveStop = resolve; }) : Promise.resolve(cmd === 'delegation_read' ? { task, events: [], next_cursor: 0, has_more: false } : [task]));
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  render(<Card task={task} paneId="pane" writable onOpen={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  const stop = screen.getByRole('button', { name: 'Stop task' });
  expect(stop).toHaveTextContent('Requesting stop…');
  expect(stop).toBeDisabled();
  if (expected) expect(screen.getByText(expected)).toBeInTheDocument();
  else expect(screen.queryByText('Actual recorded result')).not.toBeInTheDocument();
  expect(screen.queryByText(/Waiting for question|Waiting for approval/)).not.toBeInTheDocument();
  expect(screen.queryByText('stream_event message_stop')).not.toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_read')).toHaveLength(0);
  expect(native.invoke.mock.calls.filter(([cmd]) => cmd === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: task.id }]]);
  await act(async () => resolveStop(task));
});

it('separates reconnect from execution, has independent controls and stops only this task after readback', async () => {
  const Card = await component();
  const task = taskFixture({ connection_error: 'SSH disconnected', remote: remoteFixture() });
  let resolveStop!: (task: LocalTask) => void;
  const stopping = taskFixture({ status: 'stopping', cancel_requested: true });
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_cancel') return new Promise<LocalTask>(resolve => { resolveStop = resolve; });
    if (command === 'delegation_read') return Promise.resolve({ task: stopping, events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([task]);
  });
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.subscriptionHealth).toBe('connected'));
  const open = vi.fn();
  const view = render(<Card task={task} paneId="pane" writable onOpen={open} />);
  expect(screen.getByText('Build host')).toBeInTheDocument();
  expect(screen.getByText('Running')).toBeInTheDocument();
  expect(screen.getByText(/Reconnecting/)).toBeInTheDocument();
  expect(screen.getByText('Reading route handlers')).toBeInTheDocument();
  expect(view.container.querySelector('button button')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'View task' }));
  expect(open).toHaveBeenCalledWith(task);
  const stop = screen.getByRole('button', { name: 'Stop task' });
  fireEvent.click(stop);
  fireEvent.click(stop);
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toEqual([['delegation_cancel', { paneId: 'pane', taskId: 'task-1' }]]);
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  await act(async () => { resolveStop(stopping); });
  await waitFor(() => expect(owner.result.current.tasks[0].status).toBe('stopping'));
  expect(native.invoke).toHaveBeenCalledWith('delegation_read', { paneId: 'pane', taskId: 'task-1', cursor: 0 });
  view.rerender(<Card task={taskFixture({ status: 'cancelled', remote: remoteFixture({ status: 'cancelled' }) })} paneId="pane" writable onOpen={open} />);
  expect(screen.getByText('Stopped')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Stop task' })).not.toBeInTheDocument();
});

it('keeps a rejected stop honest, is keyboard navigable and fails closed for stale or readonly ownership', async () => {
  const Card = await component();
  const task = taskFixture({ remote: remoteFixture() });
  native.invoke.mockImplementation(command => command === 'delegation_cancel' ? Promise.reject(new Error('SSH unavailable')) : Promise.resolve([task]));
  const owner = renderHook(() => useDelegationTasks('pane', 'thread-1'));
  await waitFor(() => expect(owner.result.current.loading).toBe(false));
  const open = vi.fn();
  const view = render(<Card task={task} paneId="pane" writable onOpen={open} />);
  const user = userEvent.setup();
  await user.tab();
  expect(screen.getByRole('button', { name: 'View task' })).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(open).toHaveBeenCalledWith(task);
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  await screen.findByRole('alert');
  expect(screen.getByRole('alert')).toHaveTextContent('Stop unconfirmed: SSH unavailable');
  expect(screen.getByText('Running')).toBeInTheDocument();
  expect(screen.getByText('Reading route handlers')).toBeInTheDocument();
  expect(screen.queryByText('Stopped')).not.toBeInTheDocument();
  view.rerender(<Card task={task} paneId="pane" writable={false} onOpen={open} />);
  expect(screen.getByRole('button', { name: 'Retry stop' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry stop' }));
  view.rerender(<Card task={taskFixture({ ...task, parent_thread_id: 'foreign' })} paneId="pane" writable onOpen={open} />);
  expect(screen.getByRole('button', { name: 'Stop task' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Stop task' }));
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_cancel')).toHaveLength(1);
  expect(native.listen).toHaveBeenCalledTimes(1);
});
