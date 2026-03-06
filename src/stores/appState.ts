import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface TerminalSessionSnapshot {
    session_id: string;
    title: string;
    shell: string | null;
    cwd: string;
    cols: number;
    rows: number;
    state: 'starting' | 'ready' | 'exited' | 'failed';
    last_message: string | null;
    exit_code: number | null;
}

export interface BrowserSessionSnapshot {
    browser_id: string;
    title: string;
    current_url: string | null;
    history: string[];
    history_index: number;
    is_loading: boolean;
    last_error: string | null;
    reload_nonce: number;
    last_screenshot_path: string | null;
}

export interface NotificationSnapshot {
    notification_id: string;
    workspace_id: string;
    pane_id: string | null;
    session_id: string | null;
    level: 'info' | 'attention';
    message: string;
    read: boolean;
    created_at_ms: number;
}

export type BrowserAutomationAction =
    | { kind: 'open_url'; url: string }
    | { kind: 'dom_snapshot' }
    | { kind: 'accessibility_snapshot' }
    | { kind: 'click'; selector: string }
    | { kind: 'fill'; selector: string; value: string }
    | { kind: 'type_text'; text: string }
    | { kind: 'scroll'; x: number; y: number }
    | { kind: 'evaluate'; script: string }
    | { kind: 'screenshot' }
    | { kind: 'console_logs' };

export interface BrowserAutomationResult {
    request_id: string;
    browser_id: string;
    data: unknown;
    message: string | null;
}

export type MemorySource = 'human' | 'system';
export type MemoryEntryKind = 'pinned_context' | 'decision' | 'next_step' | 'session_summary';

export interface MemoryEntry {
    entry_id: string;
    kind: MemoryEntryKind;
    source: MemorySource;
    content: string;
    tags: string[];
    tool_name: string | null;
    session_label: string | null;
    created_at_ms: number;
}

export interface ProjectMemorySnapshot {
    schema_version: number;
    project_root: string;
    project_name: string;
    project_brief: string | null;
    current_goal: string | null;
    current_focus: string | null;
    constraints: string[];
    pinned_context: MemoryEntry[];
    recent_decisions: MemoryEntry[];
    next_steps: MemoryEntry[];
    session_summaries: MemoryEntry[];
    updated_at_ms: number;
}

export interface ProjectMemoryUpdate {
    project_brief?: string | null;
    current_goal?: string | null;
    current_focus?: string | null;
    constraints?: string[] | null;
}

export interface HandoffPacket {
    project_name: string;
    project_root: string;
    summary: string;
    suggested_prompt: string;
    current_goal: string | null;
    current_focus: string | null;
    constraints: string[];
    pinned_context: string[];
    recent_decisions: string[];
    next_steps: string[];
}

export type OpenFlowRole =
    | 'orchestrator'
    | 'planner'
    | 'builder'
    | 'reviewer'
    | 'tester'
    | 'debugger'
    | 'researcher';

export interface OpenFlowTaskNode {
    task_id: string;
    title: string;
    description: string;
    role: OpenFlowRole;
    status: 'pending' | 'ready' | 'in_progress' | 'blocked' | 'passed' | 'failed' | 'cancelled';
    depends_on: string[];
    success_criteria: string[];
    produced_artifacts: string[];
}

export interface OpenFlowArtifact {
    artifact_id: string;
    kind: 'plan' | 'log' | 'screenshot' | 'diff' | 'review_note' | 'test_result' | 'browser_evidence';
    title: string;
    location: string | null;
    summary: string;
}

export interface OpenFlowTimelineEntry {
    entry_id: string;
    level: 'info' | 'warning' | 'error';
    message: string;
}

export interface OpenFlowWorkerState {
    role: OpenFlowRole;
    assigned_task_ids: string[];
    status: string;
    last_output: string | null;
}

export interface OpenFlowRetryPolicy {
    max_attempts: number;
    current_attempt: number;
    backoff_seconds: number;
}

export interface OpenFlowRunRecord {
    run_id: string;
    title: string;
    goal: string;
    status: 'draft' | 'planning' | 'executing' | 'verifying' | 'reviewing' | 'awaiting_approval' | 'completed' | 'failed' | 'cancelled';
    current_phase: string;
    replan_count: number;
    assigned_roles: OpenFlowRole[];
    task_graph: OpenFlowTaskNode[];
    artifacts: OpenFlowArtifact[];
    approvals: Array<{ checkpoint_id: string; kind: string; title: string; required: boolean; reason: string }>;
    timeline: OpenFlowTimelineEntry[];
    workers: OpenFlowWorkerState[];
    retry_policy: OpenFlowRetryPolicy;
    resumable: boolean;
}

export interface OpenFlowRuntimeSnapshot {
    active_runs: OpenFlowRunRecord[];
}

export interface OpenFlowCreateRunRequest {
    title: string;
    goal: string;
}

export type PaneNodeSnapshot =
    | {
            kind: 'terminal';
            pane_id: string;
            session_id: string;
            title: string;
      }
    | {
            kind: 'browser';
            pane_id: string;
            browser_id: string;
            title: string;
      }
    | {
            kind: 'split';
            pane_id: string;
            direction: 'horizontal' | 'vertical';
            child_sizes: number[];
            children: PaneNodeSnapshot[];
      };

export interface SurfaceSnapshot {
    surface_id: string;
    title: string;
    root: PaneNodeSnapshot;
    active_pane_id: string;
}

export interface WorkspaceSnapshot {
    workspace_id: string;
    title: string;
    cwd: string;
    git_branch: string | null;
    notification_count: number;
    latest_agent_state: string | null;
    active_surface_id: string;
    surfaces: SurfaceSnapshot[];
}

export interface PersistenceSchema {
    schema_version: number;
    stores_layout_metadata: boolean;
    stores_terminal_metadata: boolean;
    stores_live_process_state: boolean;
}

export interface CodemuxConfigSnapshot {
    config_version: number;
    default_shell: string | null;
    theme_source: string;
    linux_first: boolean;
    notification_sound_enabled: boolean;
}

export interface AppStateSnapshot {
    schema_version: number;
    active_workspace_id: string;
    workspaces: WorkspaceSnapshot[];
    terminal_sessions: TerminalSessionSnapshot[];
    browser_sessions: BrowserSessionSnapshot[];
    notifications: NotificationSnapshot[];
    persistence: PersistenceSchema;
    config: CodemuxConfigSnapshot;
}

export const appState = writable<AppStateSnapshot | null>(null);
export const projectMemory = writable<ProjectMemorySnapshot | null>(null);
export const openflowRuntime = writable<OpenFlowRuntimeSnapshot | null>(null);

export async function initAppState() {
    try {
        const snapshot = await invoke<AppStateSnapshot>('get_app_state');
        appState.set(snapshot);
    } catch (error) {
        console.error('Failed to fetch app state:', error);
    }

    await listen<AppStateSnapshot>('app-state-changed', (event) => {
        appState.set(event.payload);
    });
}

export async function initProjectMemory() {
    try {
        const snapshot = await invoke<ProjectMemorySnapshot>('get_project_memory_snapshot');
        projectMemory.set(snapshot);
    } catch (error) {
        console.error('Failed to fetch project memory:', error);
    }
}

export async function initOpenFlowRuntime() {
    try {
        const snapshot = await invoke<OpenFlowRuntimeSnapshot>('get_openflow_runtime_snapshot');
        openflowRuntime.set(snapshot);
    } catch (error) {
        console.error('Failed to fetch OpenFlow runtime:', error);
    }
}

export async function createTerminalSession() {
    return invoke<string>('create_terminal_session');
}

export async function activateTerminalSession(sessionId: string) {
    return invoke('activate_terminal_session', { sessionId });
}

export async function closeTerminalSession(sessionId: string) {
    return invoke<string>('close_terminal_session', { sessionId });
}

export async function restartTerminalSession(sessionId: string) {
    return invoke('restart_terminal_session', { sessionId });
}

export async function createWorkspace() {
    return invoke<string>('create_workspace');
}

export async function activateWorkspace(workspaceId: string) {
    return invoke('activate_workspace', { workspaceId });
}

export async function renameWorkspace(workspaceId: string, title: string) {
    return invoke('rename_workspace', { workspaceId, title });
}

export async function closeWorkspace(workspaceId: string) {
    return invoke<string>('close_workspace', { workspaceId });
}

export async function cycleWorkspace(step: number) {
    return invoke<string>('cycle_workspace', { step });
}

export async function splitPane(paneId: string, direction: 'horizontal' | 'vertical') {
    return invoke<string>('split_pane', { paneId, direction });
}

export async function activatePane(paneId: string) {
    return invoke('activate_pane', { paneId });
}

export async function cyclePane(step: number) {
    return invoke<string>('cycle_pane', { step });
}

export async function closePane(paneId: string) {
    return invoke<string | null>('close_pane', { paneId });
}

export async function resizeSplit(paneId: string, childSizes: number[]) {
    return invoke('resize_split', { paneId, childSizes });
}

export async function resizeActivePane(delta: number) {
    return invoke('resize_active_pane', { delta });
}

export async function notifyAttention(message: string, sessionId?: string, paneId?: string) {
    return invoke<string>('notify_attention', { message, sessionId, paneId });
}

export async function markWorkspaceNotificationsRead(workspaceId: string) {
    return invoke('mark_workspace_notifications_read', { workspaceId });
}

export async function setNotificationSoundEnabled(enabled: boolean) {
    return invoke('set_notification_sound_enabled', { enabled });
}

export async function createBrowserPane(paneId: string) {
    return invoke<string>('create_browser_pane', { paneId });
}

export async function browserOpenUrl(browserId: string, url: string) {
    return invoke('browser_open_url', { browserId, url });
}

export async function browserHistoryBack(browserId: string) {
    return invoke('browser_history_back', { browserId });
}

export async function browserHistoryForward(browserId: string) {
    return invoke('browser_history_forward', { browserId });
}

export async function browserReload(browserId: string) {
    return invoke('browser_reload', { browserId });
}

export async function browserSetLoadingState(browserId: string, isLoading: boolean, error?: string | null) {
    return invoke('browser_set_loading_state', { browserId, isLoading, error });
}

export async function browserCaptureScreenshot(browserId: string) {
    return invoke<string>('browser_capture_screenshot', { browserId });
}

export async function browserAutomationRun(browserId: string, action: BrowserAutomationAction) {
    return invoke<BrowserAutomationResult>('browser_automation_run', { browserId, action });
}

export async function browserAutomationComplete(requestId: string, result: BrowserAutomationResult | null, error?: string) {
    if (error) {
        return invoke('browser_automation_complete', {
            requestId,
            result: { Err: error }
        });
    }

    return invoke('browser_automation_complete', {
        requestId,
        result: { Ok: result }
    });
}

export async function updateProjectMemory(update: ProjectMemoryUpdate) {
    const snapshot = await invoke<ProjectMemorySnapshot>('update_project_memory_snapshot', { update });
    projectMemory.set(snapshot);
    return snapshot;
}

export async function addProjectMemoryEntry(
    kind: MemoryEntryKind,
    content: string,
    options?: { source?: MemorySource; tags?: string[]; toolName?: string; sessionLabel?: string }
) {
    const snapshot = await invoke<ProjectMemorySnapshot>('add_project_memory_entry', {
        kind,
        source: options?.source ?? 'human',
        content,
        tags: options?.tags ?? [],
        toolName: options?.toolName,
        sessionLabel: options?.sessionLabel
    });
    projectMemory.set(snapshot);
    return snapshot;
}

export async function generateProjectHandoff() {
    return invoke<HandoffPacket>('generate_project_handoff');
}

export async function createOpenFlowRun(request: OpenFlowCreateRunRequest) {
    const run = await invoke<OpenFlowRunRecord>('create_openflow_run', { request });
    const snapshot = await invoke<OpenFlowRuntimeSnapshot>('get_openflow_runtime_snapshot');
    openflowRuntime.set(snapshot);
    return run;
}

export async function advanceOpenFlowRunPhase(runId: string) {
    const run = await invoke<OpenFlowRunRecord>('advance_openflow_run_phase', { runId });
    const snapshot = await invoke<OpenFlowRuntimeSnapshot>('get_openflow_runtime_snapshot');
    openflowRuntime.set(snapshot);
    return run;
}

export async function retryOpenFlowRun(runId: string) {
    const run = await invoke<OpenFlowRunRecord>('retry_openflow_run', { runId });
    const snapshot = await invoke<OpenFlowRuntimeSnapshot>('get_openflow_runtime_snapshot');
    openflowRuntime.set(snapshot);
    return run;
}
