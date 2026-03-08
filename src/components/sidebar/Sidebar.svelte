<script lang="ts">
    import {
        appState,
        activateWorkspace,
        closeWorkspace,
        markWorkspaceNotificationsRead,
        renameWorkspace,
        createTerminalSession,
        notifyAttention,
        setNotificationSoundEnabled,
        type WorkspaceTemplateKind,
        type LayoutPreset
    } from '../../stores/appState';
    import { collectWorkspaceSessionIds, findActiveSessionId } from '../../lib/paneTree';
    import WorkspaceRow from './WorkspaceRow.svelte';
    import NotificationsSection from './NotificationsSection.svelte';
    import OpenFlowLauncher from './OpenFlowLauncher.svelte';
    import MemoryDrawer from './MemoryDrawer.svelte';
    import NewWorkspaceLauncher from './NewWorkspaceLauncher.svelte';

    let { windowFocused }: { windowFocused: boolean } = $props();

    let renamingWorkspaceId = $state<string | null>(null);
    let renameDraft = $state('');
    let renameInputEl = $state<HTMLInputElement | null>(null);
    let showingLauncher = $state(false);
    let launcherKind = $state<WorkspaceTemplateKind>('codemux');
    let launcherLayout = $state<LayoutPreset>('single');

    $effect(() => {
        if (renamingWorkspaceId && renameInputEl) {
            renameInputEl.focus();
            renameInputEl.select();
        }
    });

    const activeWorkspace = $derived(
        $appState?.workspaces.find((w) => w.workspace_id === $appState?.active_workspace_id) ?? null
    );

    const activeWorkspaceSurface = $derived(
        activeWorkspace?.surfaces.find((s) => s.surface_id === activeWorkspace.active_surface_id) ?? null
    );

    function activeSessionId() {
        return findActiveSessionId(activeWorkspaceSurface) ?? $appState?.terminal_sessions[0]?.session_id ?? null;
    }

    function sessionsForWorkspace(workspaceId: string) {
        const workspace = $appState?.workspaces.find((w) => w.workspace_id === workspaceId);
        if (!workspace || !$appState) return [];
        const ids = collectWorkspaceSessionIds(workspace.surfaces);
        return $appState.terminal_sessions.filter((s) => ids.has(s.session_id));
    }

    function compactPath(path: string) {
        const parts = path.split(/[\\/]/).filter(Boolean);
        if (parts.length <= 3) return path;
        return `~/${parts.slice(-2).join('/')}`;
    }

    async function handleCreateWorkspace() {
        launcherKind = 'codemux';
        launcherLayout = 'single';
        showingLauncher = true;
    }

    function handleOpenFlowLauncher() {
        launcherKind = 'openflow';
        launcherLayout = 'single';
        showingLauncher = true;
    }

    async function handleActivateWorkspace(workspaceId: string) {
        try {
            await activateWorkspace(workspaceId);
        } catch (error) {
            console.error('Failed to activate workspace:', error);
        }
    }

    async function handleCloseWorkspace(workspaceId: string) {
        try {
            await closeWorkspace(workspaceId);
        } catch (error) {
            console.error('Failed to close workspace:', error);
        }
    }

    async function handleMarkRead(workspaceId: string) {
        try {
            await markWorkspaceNotificationsRead(workspaceId);
        } catch (error) {
            console.error('Failed to mark read:', error);
        }
    }

    function startRename(workspaceId: string, currentTitle: string) {
        renamingWorkspaceId = workspaceId;
        renameDraft = currentTitle;
    }

    async function commitRename() {
        if (!renamingWorkspaceId || !renameDraft.trim()) {
            renamingWorkspaceId = null;
            return;
        }
        try {
            await renameWorkspace(renamingWorkspaceId, renameDraft.trim());
        } catch (error) {
            console.error('Failed to rename workspace:', error);
        } finally {
            renamingWorkspaceId = null;
        }
    }

    async function handleCreateSession() {
        try {
            await createTerminalSession();
        } catch (error) {
            console.error('Failed to create session:', error);
        }
    }

    async function handleTestNotification() {
        const sessionId = activeSessionId() ?? undefined;
        const paneId = activeWorkspaceSurface?.active_pane_id;
        try {
            await notifyAttention('Agent needs your input', sessionId, paneId);
        } catch (error) {
            console.error('Failed to test notification:', error);
        }
    }

    async function handleSoundToggle(enabled: boolean) {
        try {
            await setNotificationSoundEnabled(enabled);
        } catch (error) {
            console.error('Failed to set notification sound:', error);
        }
    }
</script>

<aside class="sidebar">
    {#if showingLauncher}
        <NewWorkspaceLauncher
            initialKind={launcherKind}
            initialLayout={launcherLayout}
            on:close={() => (showingLauncher = false)}
        />
    {/if}

    <!-- Brand + active workspace header -->
    <header class="sidebar-head">
        <div class="brand-row">
            <div class="brand-mark">
                <span class="brand-diamond"></span>
                <span class="brand-name">Codemux</span>
            </div>
            <button
                class="icon-btn"
                type="button"
                title="New workspace"
                onclick={handleCreateWorkspace}
                aria-label="New workspace"
            >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M6 1v10M1 6h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                </svg>
            </button>
        </div>

        {#if activeWorkspace}
            <div class="active-workspace-info">
                {#if renamingWorkspaceId === activeWorkspace.workspace_id}
                    <input
                        class="rename-input"
                        bind:this={renameInputEl}
                        bind:value={renameDraft}
                        onblur={commitRename}
                        onkeydown={(e) => {
                            if (e.key === 'Enter') commitRename();
                            if (e.key === 'Escape') { renamingWorkspaceId = null; }
                        }}
                    />
                {:else}
                    <button
                        class="active-name-btn"
                        type="button"
                        title="Double-click to rename"
                        ondblclick={() => startRename(activeWorkspace.workspace_id, activeWorkspace.title)}
                    >
                        <span class="active-workspace-name">{activeWorkspace.title}</span>
                        <span class="rename-hint">
                            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                <path d="M7 1.5L8.5 3 4 7.5H2.5V6L7 1.5z" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/>
                            </svg>
                        </span>
                    </button>
                {/if}
                <span class="active-workspace-path">{compactPath(activeWorkspace.cwd)}</span>
            </div>
        {/if}

        <div class="head-actions">
            <button
                class="head-action-btn"
                type="button"
                title="New shell in active workspace"
                onclick={handleCreateSession}
            >
                + shell
            </button>
            <span class="focus-indicator" class:focused={windowFocused} title={windowFocused ? 'Window focused' : 'Background'}>
                <span class="focus-dot"></span>
            </span>
        </div>
    </header>

    <div class="sidebar-divider"></div>

    <!-- Workspace list -->
    <div class="workspace-list">
        {#if $appState?.workspaces.length}
            {#each $appState.workspaces as workspace (workspace.workspace_id)}
                <WorkspaceRow
                    {workspace}
                    isActive={workspace.workspace_id === $appState.active_workspace_id}
                    onActivate={() => handleActivateWorkspace(workspace.workspace_id)}
                    onClose={() => handleCloseWorkspace(workspace.workspace_id)}
                    onMarkRead={() => handleMarkRead(workspace.workspace_id)}
                />
            {/each}
        {:else}
            <div class="empty-workspace-hint">
                <button class="create-first-btn" type="button" onclick={handleCreateWorkspace}>
                    Create first workspace
                </button>
            </div>
        {/if}
    </div>

    <div class="sidebar-spacer"></div>

    <!-- Secondary sections -->
    <div class="sidebar-sections">
        {#if activeWorkspace}
            <div class="sidebar-divider"></div>
            <NotificationsSection workspaceId={activeWorkspace.workspace_id} />
        {/if}

        <div class="sidebar-divider"></div>
        <OpenFlowLauncher on:newrun={handleOpenFlowLauncher} />

        <div class="sidebar-divider"></div>
        <MemoryDrawer />
    </div>

    <!-- Footer -->
    <footer class="sidebar-footer">
        {#if $appState}
            <label class="sound-toggle">
                <input
                    type="checkbox"
                    checked={$appState.config.notification_sound_enabled}
                    onchange={(e) => handleSoundToggle((e.currentTarget as HTMLInputElement).checked)}
                />
                <span>Alert sound</span>
            </label>
        {/if}
        <button class="footer-debug-btn" type="button" onclick={handleTestNotification} title="Test attention signal">
            Test alert
        </button>
    </footer>
</aside>

<style>
    .sidebar {
        display: flex;
        flex-direction: column;
        width: var(--ui-sidebar-width, 240px);
        min-width: 0;
        height: 100%;
        background: var(--ui-layer-1);
        border-right: 1px solid var(--ui-border-soft);
        overflow: hidden;
        flex-shrink: 0;
    }

    /* ---- Head ---- */

    .sidebar-head {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 14px 12px 10px;
        flex-shrink: 0;
    }

    .brand-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }

    .brand-mark {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .brand-diamond {
        display: inline-block;
        width: 10px;
        height: 10px;
        background: var(--ui-accent);
        transform: rotate(45deg);
        border-radius: 2px;
        flex-shrink: 0;
        box-shadow: 0 0 8px color-mix(in srgb, var(--ui-accent) 60%, transparent);
    }

    .brand-name {
        font-size: 0.82rem;
        font-weight: 700;
        color: var(--ui-text-primary);
        letter-spacing: 0.02em;
    }

    .icon-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 26px;
        height: 26px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
        flex-shrink: 0;
    }

    .icon-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 28%, transparent);
    }

    .active-workspace-info {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
    }

    .active-name-btn {
        display: flex;
        align-items: center;
        gap: 5px;
        background: transparent;
        border: none;
        padding: 0;
        cursor: default;
        min-width: 0;
        max-width: 100%;
        text-align: left;
    }

    .active-workspace-name {
        font-size: 0.9rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        flex: 1;
        min-width: 0;
    }

    .rename-hint {
        color: var(--ui-text-muted);
        opacity: 0;
        flex-shrink: 0;
        display: flex;
        align-items: center;
        transition: opacity var(--ui-motion-fast);
        cursor: pointer;
    }

    .active-name-btn:hover .rename-hint {
        opacity: 1;
    }

    .rename-input {
        width: 100%;
        box-sizing: border-box;
        background: var(--ui-layer-2);
        border: 1px solid color-mix(in srgb, var(--ui-accent) 36%, transparent);
        border-radius: 5px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.9rem;
        font-weight: 600;
        padding: 3px 7px;
        outline: none;
    }

    .active-workspace-path {
        font-size: 0.74rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .head-actions {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .head-action-btn {
        padding: 4px 9px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.72rem;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .head-action-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .focus-indicator {
        display: flex;
        align-items: center;
        gap: 5px;
        margin-left: auto;
    }

    .focus-dot {
        width: 6px;
        height: 6px;
        border-radius: 50%;
        background: var(--ui-text-muted);
        transition: background var(--ui-motion-fast);
    }

    .focus-indicator.focused .focus-dot {
        background: var(--ui-success);
        box-shadow: 0 0 5px color-mix(in srgb, var(--ui-success) 60%, transparent);
    }

    /* ---- Divider ---- */

    .sidebar-divider {
        height: 1px;
        background: var(--ui-border-soft);
        margin: 0 0;
        flex-shrink: 0;
    }

    /* ---- Workspace list ---- */

    .workspace-list {
        display: flex;
        flex-direction: column;
        gap: 1px;
        padding: 6px 6px;
        flex-shrink: 0;
    }

    .empty-workspace-hint {
        padding: 12px;
        display: flex;
        justify-content: center;
    }

    .create-first-btn {
        padding: 7px 14px;
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        border: 1px solid color-mix(in srgb, var(--ui-accent) 28%, transparent);
        border-radius: 6px;
        color: var(--ui-accent);
        font: inherit;
        font-size: 0.8rem;
        font-weight: 600;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
    }

    .create-first-btn:hover {
        background: color-mix(in srgb, var(--ui-accent) 20%, transparent);
    }

    /* ---- Spacer pushes secondary sections to bottom ---- */

    .sidebar-spacer {
        flex: 1;
        min-height: 12px;
    }

    /* ---- Secondary sections ---- */

    .sidebar-sections {
        display: flex;
        flex-direction: column;
        overflow-y: auto;
        overflow-x: hidden;
        flex-shrink: 0;
        max-height: 60vh;
    }

    .sidebar-sections::-webkit-scrollbar {
        width: 4px;
    }

    .sidebar-sections::-webkit-scrollbar-track {
        background: transparent;
    }

    .sidebar-sections::-webkit-scrollbar-thumb {
        background: var(--ui-border-soft);
        border-radius: 2px;
    }

    /* ---- Footer ---- */

    .sidebar-footer {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 8px 12px;
        border-top: 1px solid var(--ui-border-soft);
        flex-shrink: 0;
    }

    .sound-toggle {
        display: flex;
        align-items: center;
        gap: 6px;
        cursor: pointer;
        flex: 1;
    }

    .sound-toggle input {
        width: 14px;
        height: 14px;
        cursor: pointer;
        accent-color: var(--ui-accent);
        flex-shrink: 0;
    }

    .sound-toggle span {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
    }

    .footer-debug-btn {
        padding: 3px 7px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 4px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.68rem;
        cursor: pointer;
        white-space: nowrap;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .footer-debug-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }
</style>
