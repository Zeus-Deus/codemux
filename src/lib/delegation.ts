import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AgentChatProviderKind, ProviderChatCapabilities } from '@/tauri/types';
import type { ApprovalDecision, ProviderRuntimeEvent } from '@/tauri/events';
import type { ChatViewItem } from '@/lib/agent-chat/types';

export type DelegationProvider = 'codex' | 'claude';
export type TaskStatus = 'starting' | 'running' | 'awaiting_approval' | 'stopping' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
export type WakeState = 'pending' | 'delivering' | 'delivered' | 'held' | 'suppressed';
export interface DeliveryProjection {
  outcome: 'not_attempted' | 'before_send' | 'in_flight' | 'accepted' | 'unknown';
  eligible: boolean;
  reason: 'cancelled' | 'no_result' | 'not_terminal' | 'in_flight' | 'accepted' | 'unknown' | 'already_delivered' | 'tail_pending' | 'authority_unavailable' | null;
}
export interface DelegationWorkspace { id: string; name: string; path: string }
export interface DelegationHostInfo {
  protocol_version: 1;
  providers: { provider: DelegationProvider; capabilities: ProviderChatCapabilities | null; error: string | null }[];
  workspaces: DelegationWorkspace[];
}
export interface Grant {
  id: string;
  host_id: number;
  host_name: string;
  ssh_target: string;
  workspace_path: string;
  workspace_name: string;
  provider: DelegationProvider;
  permission_mode: string;
  enabled: boolean;
  created_at: string;
}
export interface AuthorizeDelegationInput {
  host_id: number;
  workspace_path: string;
  provider: DelegationProvider;
  permission_mode: string;
}
export interface DelegateTaskInput {
  target_id: string;
  prompt: string;
  title?: string;
  model?: string | null;
  effort?: string | null;
  client_request_id: string;
}
export interface LaunchRequest {
  id: string;
  parent_thread_id: string;
  parent_label: string;
  workspace_path: string;
  prompt: string;
  provider: DelegationProvider;
  model: string | null;
  permission_mode: string;
  effort: string | null;
}
export interface RemoteApproval { request_id: string; request_kind: string; payload: unknown }
export interface TaskSnapshot {
  id: string;
  request: LaunchRequest;
  child_thread_id: string;
  provider_session_id: string | null;
  turn_id: string | null;
  status: TaskStatus;
  activity: string | null;
  result: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
  cancel_requested: boolean;
  pending_requests: RemoteApproval[];
}
export interface LocalTask {
  id: string;
  parent_thread_id: string;
  parent_provider: AgentChatProviderKind;
  parent_workspace_id: string;
  parent_event_id: number | null;
  target_host_id: number;
  target_host_name: string;
  target_ssh_target: string;
  target_workspace_id: string;
  target_workspace_path: string;
  title: string;
  prompt: string;
  provider: DelegationProvider;
  model: string | null;
  permission_mode: string;
  created_at: string;
  updated_at: string;
  status: TaskStatus;
  connection_error: string | null;
  cancel_requested: boolean;
  remote: TaskSnapshot | null;
  wake_state: WakeState;
  wake_error: string | null;
  delivery: DeliveryProjection;
  responses: ResponseProjection[];
}
export interface ResponseProjection {
  request_id: string;
  outcome: DeliveryProjection['outcome'];
  decision: ApprovalDecision | null;
  eligible: boolean;
  reason: 'cancelled' | 'in_flight' | 'accepted' | 'unknown' | 'tail_pending' | 'authority_unavailable' | 'payload_changed' | null;
}
export interface TaskEvent { sequence: number; event: ProviderRuntimeEvent }
export interface TaskRead { task: LocalTask; events: TaskEvent[]; next_cursor: number; has_more: boolean }
export interface RemoteTaskItem { kind: 'remote_task'; id: string; seq: number; task: LocalTask }

export const delegationHostInfo = (hostId: number) => invoke<DelegationHostInfo>('delegation_host_info', { hostId });
export const delegationGrants = (paneId: string) => invoke<Grant[]>('delegation_grants', { paneId });
export const delegationAuthorize = (paneId: string, input: AuthorizeDelegationInput) => invoke<Grant>('delegation_authorize', { paneId, input });
export const delegationRevoke = (paneId: string, targetId: string) => invoke<void>('delegation_revoke', { paneId, targetId });
export const delegateTask = (paneId: string, input: DelegateTaskInput) => invoke<LocalTask>('delegate_task', { paneId, input });
export const delegationList = (paneId: string) => invoke<LocalTask[]>('delegation_list', { paneId });
export const delegationRead = (paneId: string, taskId: string, cursor = 0) => invoke<TaskRead>('delegation_read', { paneId, taskId, cursor });
export const delegationCancel = (paneId: string, taskId: string) => invoke<LocalTask>('delegation_cancel', { paneId, taskId });
export const delegationRespond = (paneId: string, taskId: string, requestId: string, decision: ApprovalDecision, originalRequest: RemoteApproval) => invoke<LocalTask>('delegation_respond', { paneId, taskId, requestId, decision, originalRequest });
export const delegationDeliver = (paneId: string, taskId: string) => invoke<LocalTask>('delegation_deliver', { paneId, taskId });
export const onDelegationChanged = (callback: (payload: { parent_thread_id: string }) => void): Promise<UnlistenFn> =>
  listen<{ parent_thread_id: string }>('delegation-changed', (event) => callback(event.payload));

/** Inserts app-owned receipts after their durable parent event. Parent rows
 * retain both their identity and order; missing anchors remain visible at tail.
 * Status/updated_at never affect placement, including settled/attention tasks. */
export function mergeDelegationItems(messages: readonly ChatViewItem[], tasks: readonly LocalTask[]): (ChatViewItem | RemoteTaskItem)[] {
  if (!tasks.length) return messages as ChatViewItem[];
  const unique = [...new Map(tasks.map(task => [task.id, task])).values()]
    .sort((a, b) => a.created_at.localeCompare(b.created_at) || a.id.localeCompare(b.id));
  const anchors = new Map<number, number>();
  messages.forEach((message, index) => {
    if ('source_event_id' in message && message.source_event_id != null) anchors.set(message.source_event_id, index);
  });
  const buckets = new Map<number, LocalTask[]>();
  for (const task of unique) {
    const index = task.parent_event_id == null ? undefined : anchors.get(task.parent_event_id);
    const key = index ?? messages.length;
    buckets.set(key, [...(buckets.get(key) ?? []), task]);
  }
  const result: (ChatViewItem | RemoteTaskItem)[] = [];
  const emit = (index: number, base: number, gap: number) => {
    const bucket = buckets.get(index) ?? [];
    bucket.forEach((task, position) => result.push({ kind: 'remote_task', id: `remote-task:${task.id}`, seq: base + gap * (position + 1) / (bucket.length + 1), task }));
  };
  messages.forEach((message, index) => {
    result.push(message);
    emit(index, message.seq, Math.max(Number.EPSILON, (messages[index + 1]?.seq ?? message.seq + 1) - message.seq));
  });
  emit(messages.length, Math.max(-1, ...messages.map(message => message.seq)) + 1, 1);
  return result;
}
