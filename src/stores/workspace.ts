import { invoke } from '@tauri-apps/api/core';
import { appState, syncAppState } from './core';
import type {
    AppStateSnapshot,
    EditorInfo,
    LayoutPreset,
    TabKind,
    WorkspaceConfig,
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

export async function closeWorkspace(workspaceId: string, forceDelete = false) {
    return invoke<string>('close_workspace', { workspaceId, forceDelete });
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

export async function killPort(port: number) {
    return invoke('kill_port', { port });
}

export async function detectEditors() {
    return invoke<EditorInfo[]>('detect_editors');
}

export async function openInEditor(editorId: string, path: string) {
    return invoke<void>('open_in_editor', { editorId, path });
}

export async function createWorktreeWorkspace(
    repoPath: string,
    branch: string,
    newBranch: boolean,
    layout: string,
    base?: string | null,
) {
    const workspaceId = await invoke<string>('create_worktree_workspace', {
        repoPath,
        branch,
        newBranch,
        base: base ?? null,
        layout,
    });
    await activateWorkspace(workspaceId);
    await syncAppState();
    return workspaceId;
}

export async function importWorktreeWorkspace(
    worktreePath: string,
    branch: string,
    layout: string,
) {
    const workspaceId = await invoke<string>('import_worktree_workspace', {
        worktreePath,
        branch,
        layout,
    });
    await activateWorkspace(workspaceId);
    await syncAppState();
    return workspaceId;
}

export async function closeWorkspaceWithWorktree(workspaceId: string, removeWorktree: boolean, forceDelete = false) {
    return invoke<void>('close_workspace_with_worktree', { workspaceId, removeWorktree, forceDelete });
}

export async function getWorkspaceConfig(path: string) {
    return invoke<WorkspaceConfig | null>('get_workspace_config', { path });
}

export async function runWorkspaceSetup(workspaceId: string) {
    return invoke<void>('run_workspace_setup', { workspaceId });
}

// ---- Workspace sections ----

export async function createSection(name: string, color: string) {
    return invoke<string>('create_section', { name, color });
}

export async function renameSection(sectionId: string, name: string) {
    return invoke('rename_section', { sectionId, name });
}

export async function deleteSection(sectionId: string) {
    return invoke('delete_section', { sectionId });
}

export async function setSectionColor(sectionId: string, color: string) {
    return invoke('set_section_color', { sectionId, color });
}

export async function toggleSectionCollapsed(sectionId: string) {
    return invoke('toggle_section_collapsed', { sectionId });
}

export async function moveWorkspaceToSection(workspaceId: string, sectionId: string | null, position?: number) {
    return invoke('move_workspace_to_section', { workspaceId, sectionId, position: position ?? null });
}

export async function reorderWorkspaces(workspaceIds: string[]) {
    return invoke('reorder_workspaces', { workspaceIds });
}

export async function reorderSections(sectionIds: string[]) {
    return invoke('reorder_sections', { sectionIds });
}
