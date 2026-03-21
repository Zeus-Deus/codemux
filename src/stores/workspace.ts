import { invoke } from '@tauri-apps/api/core';
import { appState, syncAppState } from './core';
import type {
    AppStateSnapshot,
    LayoutPreset,
    TabKind,
    WorkspaceTemplateKind,
} from './types';

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
    return invoke<string>('create_workspace', { cwd: null });
}

export async function createWorkspaceAtPath(cwd: string) {
    return invoke<string>('create_workspace', { cwd });
}

export async function updateWorkspaceCwd(workspaceId: string, cwd: string) {
    return invoke('update_workspace_cwd', { workspaceId, cwd });
}

export async function createOpenFlowWorkspace(title: string, goal: string, cwd?: string) {
    return invoke<string>('create_openflow_workspace', { title, goal, cwd: cwd || null });
}

export async function createWorkspaceWithPreset(options: {
    kind: WorkspaceTemplateKind;
    layout: LayoutPreset;
    cwd?: string | null;
    openflowTitle?: string;
    openflowGoal?: string;
}) {
    let workspaceId: string;

    if (options.kind === 'openflow') {
        workspaceId = await invoke<string>('create_openflow_workspace', {
            title: options.openflowTitle || 'OpenFlow',
            goal: options.openflowGoal || '',
            cwd: options.cwd?.trim() ? options.cwd.trim() : null,
        });
    } else {
        workspaceId = await invoke<string>('create_workspace_with_preset', {
            cwd: options.cwd?.trim() ? options.cwd.trim() : null,
            layout: options.layout,
        });
    }

    await activateWorkspace(workspaceId);
    await syncAppState();

    return { workspaceId, runId: null };
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

export async function swapPanes(sourcePaneId: string, targetPaneId: string) {
    return invoke('swap_panes', { sourcePaneId, targetPaneId });
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

export async function createTab(workspaceId: string, kind: TabKind) {
    return invoke<string>('create_tab', { workspaceId, kind });
}

export async function closeTab(workspaceId: string, tabId: string) {
    return invoke('close_tab', { workspaceId, tabId });
}

export async function activateTab(workspaceId: string, tabId: string) {
    return invoke('activate_tab', { workspaceId, tabId });
}

export async function renameTab(workspaceId: string, tabId: string, title: string) {
    return invoke('rename_tab', { workspaceId, tabId, title });
}

export async function refreshWorkspaceState() {
    const snapshot = await invoke<AppStateSnapshot>('get_app_state');
    appState.set(snapshot);
    return snapshot;
}
