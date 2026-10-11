import { afterEach, expect, it, vi } from 'vitest';
import type { ChatViewItem } from '@/lib/agent-chat/types';
import { taskFixture } from './delegation.test-fixtures';
import * as delegation from './delegation';
const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(null));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
afterEach(() => { vi.clearAllMocks(); });

it('exposes pane-scoped native wrappers with exact command arguments', async () => {
  const modules = import.meta.glob('./delegation.ts');
  expect(modules['./delegation.ts'], 'production delegation API must exist').toBeTypeOf('function');
  const api = await modules['./delegation.ts']() as Record<string, (...args: unknown[]) => Promise<unknown>>;
  await api.delegationHostInfo(7);
  expect(invoke).toHaveBeenLastCalledWith('delegation_host_info', { hostId: 7 });
  await api.delegationGrants('pane');
  expect(invoke).toHaveBeenLastCalledWith('delegation_grants', { paneId: 'pane' });
  const input = { host_id: 7, workspace_path: '/srv/project', provider: 'codex', permission_mode: 'review' };
  await api.delegationAuthorize('pane', input);
  expect(invoke).toHaveBeenLastCalledWith('delegation_authorize', { paneId: 'pane', input });
  await api.delegationRevoke('pane', 'grant');
  expect(invoke).toHaveBeenLastCalledWith('delegation_revoke', { paneId: 'pane', targetId: 'grant' });
  const launch = { target_id: 'grant', prompt: 'Audit routes', client_request_id: 'stable' };
  await api.delegateTask('pane', launch);
  expect(invoke).toHaveBeenLastCalledWith('delegate_task', { paneId: 'pane', input: launch });
  await api.delegationList('pane');
  expect(invoke).toHaveBeenLastCalledWith('delegation_list', { paneId: 'pane' });
  await api.delegationRead('pane', 'task', 19);
  expect(invoke).toHaveBeenLastCalledWith('delegation_read', { paneId: 'pane', taskId: 'task', cursor: 19 });
  await api.delegationCancel('pane', 'task');
  expect(invoke).toHaveBeenLastCalledWith('delegation_cancel', { paneId: 'pane', taskId: 'task' });
  const decision = { decision: 'deny', message: 'Denied by user' };
  const originalRequest={request_id:'approval',request_kind:'tool',payload:{exact:[null,false]}};
  await api.delegationRespond('pane', 'task', 'approval', decision, originalRequest);
  expect(invoke).toHaveBeenLastCalledWith('delegation_respond', { paneId: 'pane', taskId: 'task', requestId: 'approval', decision, originalRequest });
  await api.delegationDeliver('pane', 'task');
  expect(invoke).toHaveBeenLastCalledWith('delegation_deliver', { paneId: 'pane', taskId: 'task' });
});

it('anchors tasks to durable source events without sorting or folding parent messages', () => {
  const merge = (delegation as Record<string, unknown>).mergeDelegationItems;
  expect(merge, 'durable task projection must exist').toBeTypeOf('function');
  const messages: ChatViewItem[] = [
    { kind: 'user_message', id: 'first', seq: 2, text: 'Start', source_event_id: 10 },
    { kind: 'user_message', id: 'second', seq: 3, text: 'Later', source_event_id: 90 },
  ];
  const completed = taskFixture({ id: 'done', status: 'completed' });
  const attention = taskFixture({ id: 'attention', status: 'awaiting_approval', parent_event_id: 90 });
  const unanchored = taskFixture({ id: 'unanchored', parent_event_id: null });
  const project = merge as typeof import('./delegation').mergeDelegationItems;
  const items = project(messages, [unanchored, attention, completed]);
  expect(items.map(item => item.id)).toEqual(['first', 'remote-task:done', 'second', 'remote-task:attention', 'remote-task:unanchored']);
  expect(items[0]).toBe(messages[0]);
  expect(items[2]).toBe(messages[1]);
  expect(messages.map(item => item.id)).toEqual(['first', 'second']);
  expect(items[1].seq).toBeGreaterThan(2);
  expect(items[1].seq).toBeLessThan(3);
  expect(project(messages, [completed, attention, unanchored]).map(item => item.id)).toEqual(items.map(item => item.id));
  expect(project(messages, [completed, completed]).filter(item => item.kind === 'remote_task')).toHaveLength(1);
});
