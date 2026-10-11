import type { DelegationHostInfo, Grant, LocalTask, TaskSnapshot } from './delegation';

/** Synthetic transport fixtures only; never loaded by product code. */
export function taskFixture(overrides: Partial<LocalTask> = {}): LocalTask {
  return {
    id: 'task-1', parent_thread_id: 'thread-1', parent_provider: 'codex', parent_workspace_id: 'parent-project', parent_event_id: 10,
    target_host_id: 7, target_host_name: 'Build host', target_ssh_target: 'builder@example.test', target_workspace_id: 'checkout-1', target_workspace_path: '/srv/project',
    title: 'Audit routes', prompt: 'Audit route access', provider: 'codex', model: 'receiver-model', permission_mode: 'review',
    created_at: '2026-10-08T10:00:00Z', updated_at: '2026-10-08T10:00:00Z', status: 'running', connection_error: null, cancel_requested: false,
    remote: null, wake_state: 'pending', wake_error: null, responses: [],
    delivery: { outcome: 'not_attempted', eligible: false, reason: 'no_result' }, ...overrides,
  };
}
export function remoteFixture(overrides: Partial<TaskSnapshot> = {}): TaskSnapshot {
  return {
    id: 'task-1', request: { id: 'task-1', parent_thread_id: 'thread-1', parent_label: 'Parent', workspace_path: '/srv/project', prompt: 'Audit route access', provider: 'codex', model: 'receiver-model', permission_mode: 'review', effort: null },
    child_thread_id: 'child-1', provider_session_id: null, turn_id: 'turn-1', status: 'running', activity: 'Reading route handlers', result: null, error: null,
    created_at: '2026-10-08T10:00:00Z', updated_at: '2026-10-08T10:00:00Z', cancel_requested: false, pending_requests: [], ...overrides,
  };
}
export function receiverFixture(model = 'receiver-model'): DelegationHostInfo {
  return { protocol_version: 1, providers: [{ provider: 'codex', error: null, capabilities: {
    models: [{ id: model, label: 'Receiver model', description: null, effort_levels: ['low', 'high'], default_effort: 'low', prompt_injected_effort_levels: [], context_window_options: [], supports_adaptive_thinking: false, supports_thinking_toggle: false, supports_fast_mode: false, supports_images: false, sub_provider: null, is_free: false }],
    effort_granularity: 'per_turn', effort_label_map: { low: 'Low', high: 'High' }, permission_modes: [{ value: 'review', label: 'Review changes', description: 'Ask before changes', is_default: true }], default_permission_mode: 'review', permission_granularity: 'per_session',
  } }], workspaces: [{ id: 'checkout-1', name: 'Project checkout', path: '/srv/project' }] };
}
export const grantFixture: Grant = { id: 'grant-1', host_id: 7, host_name: 'Build host', ssh_target: 'builder@example.test', workspace_path: '/srv/project', workspace_name: 'Project checkout', provider: 'codex', permission_mode: 'review', enabled: true, created_at: '2026-10-08T10:00:00Z' };
