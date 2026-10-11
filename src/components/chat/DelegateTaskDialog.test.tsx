import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { useHostsStore, __resetHostsStoreForTests } from '@/stores/hosts-store';
import { useDelegationTasks } from '@/stores/delegation-store';
import { grantFixture, receiverFixture, taskFixture } from '@/lib/delegation.test-fixtures';
const native = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: native.listen }));
afterEach(() => { cleanup(); __resetHostsStoreForTests(); vi.clearAllMocks(); });
function hosts() {
  useHostsStore.setState({ loaded: true, hosts: [{ id: 7, name: 'Build host', ssh_target: 'builder@example.test', server_id: null, created_at: '', updated_at: '', dirty: false }], error: null });
}
async function component() {
  const modules = import.meta.glob('./DelegateTaskDialog.tsx');
  expect(modules['./DelegateTaskDialog.tsx'], 'consent dialog must exist').toBeTypeOf('function');
  return (await modules['./DelegateTaskDialog.tsx']() as typeof import('./DelegateTaskDialog')).DelegateTaskDialog;
}
it.each(['empty-hosts', 'incompatible-helper'] as const)('guides %s recovery to the actual Devices settings without authorizing or launching', async state => {
  const Dialog = await component();
  hosts();
  if (state === 'empty-hosts') useHostsStore.setState({ hosts: [] });
  native.invoke.mockImplementation(command => command === 'delegation_host_info' ? Promise.resolve({ ...receiverFixture(), protocol_version: 2 }) : Promise.resolve([]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Dialog open onOpenChange={vi.fn()} paneId="pane" threadId="thread-1" writable onLaunched={vi.fn()} />);
  if (state === 'incompatible-helper') {
    fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
    await screen.findByRole('alert');
    expect(screen.getByRole('button', { name: 'Refresh receiver' })).toBeEnabled();
  }
  await screen.findByText(/Settings → Devices/);
  expect(screen.queryByText(/Settings → Hosts/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Run on host' })).toBeDisabled();
  expect(native.invoke.mock.calls.some(([command]) => command === 'delegation_authorize' || command === 'delegate_task')).toBe(false);
});

it('shows exact receiver model/checkout/consent and Cancel never authorizes or launches', async () => {
  const Dialog = await component();
  hosts();
  native.invoke.mockImplementation(command => command === 'delegation_host_info' ? Promise.resolve(receiverFixture()) : Promise.resolve([]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const close = vi.fn();
  render(<Dialog open onOpenChange={close} paneId="pane" threadId="thread-1" writable onLaunched={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  await screen.findByRole('option', { name: 'Receiver model' });
  fireEvent.change(screen.getByLabelText('Existing checkout'), { target: { value: 'checkout-1' } });
  fireEvent.change(screen.getByLabelText('Task'), { target: { value: 'Audit route access' } });
  expect(screen.getByText('builder@example.test')).toBeInTheDocument();
  expect(screen.getByText('/srv/project')).toBeInTheDocument();
  expect(screen.getByText(/not a sandbox/i)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Run on host' })).toBeDisabled();
  fireEvent.click(screen.getByLabelText(/Authorize this host and checkout/i));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run on host' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(close).toHaveBeenCalledWith(false);
  expect(native.invoke.mock.calls.some(([command]) => command === 'delegation_authorize' || command === 'delegate_task')).toBe(false);
});

it('verifies the grant, guards duplicate clicks, preserves prompt and retries the identical launch key', async () => {
  const Dialog = await component();
  hosts();
  let rejectLaunch!: (error: Error) => void;
  let launches = 0;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_host_info') return Promise.resolve(receiverFixture());
    if (command === 'delegation_authorize') return Promise.resolve(grantFixture);
    if (command === 'delegation_grants') return Promise.resolve([grantFixture]);
    if (command === 'delegate_task') { launches++; return launches === 1 ? new Promise((_resolve, reject) => { rejectLaunch = reject; }) : Promise.resolve(taskFixture()); }
    if (command === 'delegation_read') return Promise.resolve({ task: taskFixture(), events: [], next_cursor: 0, has_more: false });
    return Promise.resolve([]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const launched = vi.fn();
  const close = vi.fn();
  render(<Dialog open onOpenChange={close} paneId="pane" threadId="thread-1" writable onLaunched={launched} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  await screen.findByRole('option', { name: 'Receiver model' });
  fireEvent.change(screen.getByLabelText('Existing checkout'), { target: { value: 'checkout-1' } });
  fireEvent.change(screen.getByLabelText('Task'), { target: { value: 'Audit route access' } });
  fireEvent.click(screen.getByLabelText(/Authorize this host and checkout/i));
  const run = screen.getByRole('button', { name: 'Run on host' });
  fireEvent.click(run);
  fireEvent.click(run);
  await waitFor(() => expect(launches).toBe(1));
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_authorize')).toHaveLength(1);
  expect(native.invoke.mock.calls.filter(([command]) => command === 'delegation_grants')).toHaveLength(1);
  await act(async () => { rejectLaunch(new Error('SSH response lost')); });
  expect(screen.getByRole('alert')).toHaveTextContent('SSH response lost');
  expect(screen.getByLabelText('Task')).toHaveValue('Audit route access');
  fireEvent.click(screen.getByRole('button', { name: 'Retry same task' }));
  await waitFor(() => expect(launched).toHaveBeenCalledTimes(1));
  const requests = native.invoke.mock.calls.filter(([command]) => command === 'delegate_task').map(([, args]) => args.input);
  expect(requests).toHaveLength(2);
  expect(requests[0]).toEqual(requests[1]);
  expect(requests[0]).toMatchObject({ target_id: 'grant-1', model: 'receiver-model', effort: 'low', prompt: 'Audit route access', client_request_id: expect.any(String) });
  expect(close).toHaveBeenCalledWith(false);
});

it('keeps the exact model and effort for an unconfirmed task across dialog close and receiver refresh', async () => {
  const Dialog = await component();
  hosts();
  let catalogueReads = 0;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_host_info') {
      catalogueReads++;
      const info = receiverFixture();
      if (catalogueReads > 2) info.providers[0].capabilities!.models.unshift({ ...receiverFixture('new-default').providers[0].capabilities!.models[0], label: 'New receiver default' });
      return Promise.resolve(info);
    }
    if (command === 'delegation_authorize') return Promise.resolve(grantFixture);
    if (command === 'delegation_grants') return Promise.resolve([grantFixture]);
    if (command === 'delegate_task') return Promise.reject(new Error('Launch acknowledgement lost'));
    return Promise.resolve([]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const props = { open: true, onOpenChange: vi.fn(), paneId: 'pane', threadId: 'thread-1', writable: true, onLaunched: vi.fn() };
  const view = render(<Dialog {...props} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  await screen.findByRole('option', { name: 'Receiver model' });
  fireEvent.change(screen.getByLabelText('Existing checkout'), { target: { value: 'checkout-1' } });
  fireEvent.change(screen.getByLabelText('Reasoning effort'), { target: { value: 'high' } });
  fireEvent.change(screen.getByLabelText('Task'), { target: { value: 'Audit route access' } });
  fireEvent.click(screen.getByLabelText(/Authorize this host and checkout/i));
  fireEvent.click(screen.getByRole('button', { name: 'Run on host' }));
  await screen.findByRole('button', { name: 'Retry same task' });
  view.rerender(<Dialog {...props} open={false} />);
  view.rerender(<Dialog {...props} />);
  await screen.findByRole('option', { name: 'New receiver default' });
  expect(screen.getByLabelText('Model')).toHaveValue('receiver-model');
  expect(screen.getByLabelText('Reasoning effort')).toHaveValue('high');
  expect(screen.getByLabelText('Task')).toHaveValue('Audit route access');
});

it.each(['offline','removed-model'] as const)('retries known task local readback despite %s capabilities without launch or renewed consent', async unavailable => {
  const Dialog = await component(); hosts();
  let launched = false; let readFailed = false;
  native.invoke.mockImplementation(command => {
    if (command==='delegation_host_info') {
      if (!launched) return Promise.resolve(receiverFixture());
      if (unavailable==='offline') return Promise.reject(new Error('Receiver offline'));
      const info=receiverFixture('replacement-model'); info.workspaces=[]; return Promise.resolve(info);
    }
    if (command==='delegation_authorize') return Promise.resolve(grantFixture);
    if (command==='delegation_grants') return Promise.resolve([grantFixture]);
    if (command==='delegate_task') {launched=true;return Promise.resolve(taskFixture());}
    if (command==='delegation_read') {if(!readFailed){readFailed=true;return Promise.reject(new Error('Local read failed'));} return Promise.resolve({task:taskFixture(),events:[],next_cursor:0,has_more:false});}
    return Promise.resolve([]);
  });
  renderHook(()=>useDelegationTasks('pane','thread-1'));
  const onLaunched=vi.fn();
  render(<Dialog open onOpenChange={vi.fn()} paneId="pane" threadId="thread-1" writable onLaunched={onLaunched} />);
  fireEvent.change(screen.getByLabelText('Host'),{target:{value:'7'}});
  await screen.findByRole('option',{name:'Receiver model'});
  fireEvent.change(screen.getByLabelText('Existing checkout'),{target:{value:'checkout-1'}});
  fireEvent.change(screen.getByLabelText('Task'),{target:{value:'Original immutable prompt'}});
  fireEvent.click(screen.getByLabelText(/Authorize this host and checkout/i));
  fireEvent.click(screen.getByRole('button',{name:'Run on host'}));
  await screen.findByRole('button',{name:'Retry same task'});
  fireEvent.click(screen.getByRole('button',{name:'Refresh receiver'}));
  await waitFor(()=>expect(screen.queryByText('Reading receiver capabilities and checkouts…')).not.toBeInTheDocument());
  expect(screen.getByLabelText(/Authorize this host and checkout/i)).not.toBeChecked();
  expect(screen.getByRole('button',{name:'Retry same task'})).toBeEnabled();
  const before=native.invoke.mock.calls.length;
  fireEvent.click(screen.getByRole('button',{name:'Retry same task'}));
  await waitFor(()=>expect(onLaunched).toHaveBeenCalledTimes(1));
  expect(native.invoke.mock.calls.slice(before)).toEqual([['delegation_read',{paneId:'pane',taskId:'task-1',cursor:0}]]);
  expect(native.invoke.mock.calls.filter(([cmd])=>cmd==='delegate_task')).toHaveLength(1);
});
it('explains an empty receiver catalogue without inventing local models or permissions', async () => {
  const Dialog = await component();
  hosts();
  const info = receiverFixture();
  info.providers[0].capabilities!.models = [];
  info.providers[0].capabilities!.permission_modes = [];
  native.invoke.mockImplementation(command => command === 'delegation_host_info' ? Promise.resolve(info) : Promise.resolve([]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Dialog open onOpenChange={vi.fn()} paneId="pane" threadId="thread-1" writable onLaunched={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  await screen.findByRole('option', { name: 'Project checkout' });
  expect(screen.getByRole('alert')).toHaveTextContent('No runnable models or permission modes');
  expect(screen.getByRole('button', { name: 'Refresh receiver' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Run on host' })).toBeDisabled();
  expect(screen.getByLabelText('Model').querySelectorAll('option')).toHaveLength(1);
});

it('fences stale receiver responses and preserves deleted host identity instead of falling back locally', async () => {
  const Dialog = await component();
  hosts();
  const first = useHostsStore.getState().hosts[0];
  useHostsStore.setState({ hosts: [first, { ...first, id: 8, name: 'Second host', ssh_target: 'second@example.test' }] });
  let resolveFirst!: (info: ReturnType<typeof receiverFixture>) => void;
  native.invoke.mockImplementation((command, args) => command === 'delegation_host_info' ? args.hostId === 7 ? new Promise(resolve => { resolveFirst = resolve; }) : Promise.resolve(receiverFixture('second-model')) : Promise.resolve([]));
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  render(<Dialog open onOpenChange={vi.fn()} paneId="pane" threadId="thread-1" writable onLaunched={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '8' } });
  await screen.findByRole('option', { name: 'Receiver model' });
  expect(screen.getByLabelText('Model')).toHaveValue('second-model');
  await act(async () => { resolveFirst(receiverFixture('stale-model')); });
  expect(screen.getByLabelText('Model')).toHaveValue('second-model');
  act(() => useHostsStore.setState({ hosts: [first] }));
  expect(screen.getByLabelText('Host')).toHaveValue('8');
  expect(screen.getByRole('alert')).toHaveTextContent('Host unavailable');
  expect(screen.getByRole('button', { name: 'Run on host' })).toBeDisabled();
  expect(native.invoke.mock.calls.some(([command]) => command === 'delegation_authorize' || command === 'delegate_task')).toBe(false);
});

it.each(['cancel', 'readonly', 'checkout-removed'] as const)('does not launch after %s supersedes receiver verification', async reason => {
  const Dialog = await component();
  hosts();
  let resolveVerification!: (info: ReturnType<typeof receiverFixture>) => void;
  let hostReads = 0;
  native.invoke.mockImplementation(command => {
    if (command === 'delegation_host_info') { hostReads++; return hostReads === 1 ? Promise.resolve(receiverFixture()) : new Promise(resolve => { resolveVerification = resolve; }); }
    return Promise.resolve([]);
  });
  renderHook(() => useDelegationTasks('pane', 'thread-1'));
  const props = { open: true, onOpenChange: vi.fn(), paneId: 'pane', threadId: 'thread-1', writable: true, onLaunched: vi.fn() };
  const view = render(<Dialog {...props} />);
  fireEvent.change(screen.getByLabelText('Host'), { target: { value: '7' } });
  await screen.findByRole('option', { name: 'Receiver model' });
  fireEvent.change(screen.getByLabelText('Existing checkout'), { target: { value: 'checkout-1' } });
  fireEvent.change(screen.getByLabelText('Task'), { target: { value: 'Audit route access' } });
  fireEvent.click(screen.getByLabelText(/Authorize this host and checkout/i));
  fireEvent.click(screen.getByRole('button', { name: 'Run on host' }));
  await waitFor(() => expect(hostReads).toBe(2));
  if (reason === 'cancel') fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  if (reason === 'readonly') view.rerender(<Dialog {...props} writable={false} />);
  const response = receiverFixture();
  if (reason === 'checkout-removed') response.workspaces = [];
  await act(async () => { resolveVerification(response); });
  expect(native.invoke.mock.calls.some(([command]) => command === 'delegation_authorize' || command === 'delegate_task')).toBe(false);
  if (reason === 'checkout-removed') expect(screen.getByRole('alert')).toHaveTextContent('no longer available');
});
