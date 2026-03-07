<script lang="ts">
    import { onMount } from 'svelte';
    import { get } from 'svelte/store';
    import { theme, fallbackTheme, initTheme } from './stores/theme';
    import {
        appState,
        initAppState,
        openflowRuntime,
        initOpenFlowRuntime,
        projectMemory,
        initProjectMemory,
        activateTerminalSession,
        createTerminalSession,
        closeTerminalSession,
        restartTerminalSession,
        createWorkspace,
        activateWorkspace,
        renameWorkspace,
        closeWorkspace,
        cycleWorkspace,
        activatePane,
        closePane,
        cyclePane,
        resizeActivePane,
        resizeSplit,
        markWorkspaceNotificationsRead,
        notifyAttention,
        setNotificationSoundEnabled,
        createBrowserPane,
        updateProjectMemory,
        addProjectMemoryEntry,
        generateProjectHandoff,
        createOpenFlowRun,
        advanceOpenFlowRunPhase,
        applyOpenFlowReviewResult,
        retryOpenFlowRun,
        runOpenFlowAutonomousLoop,
        stopOpenFlowRun,
        splitPane
    } from './stores/appState';
    import type {
        NotificationSnapshot,
        OpenFlowRunRecord,
        TerminalSessionSnapshot,
        WorkspaceSnapshot
    } from './stores/appState';
    import PaneNode from './components/panes/PaneNode.svelte';
    import { collectWorkspaceSessionIds, findActiveSessionId } from './lib/paneTree';

    const themeKeys = [
        'accent',
        'cursor',
        'foreground',
        'background',
        'selection_foreground',
        'selection_background',
        'color0',
        'color1',
        'color2',
        'color3',
        'color4',
        'color5',
        'color6',
        'color7',
        'color8',
        'color9',
        'color10',
        'color11',
        'color12',
        'color13',
        'color14',
        'color15'
    ] as const;

    let renameDraft = $state('');
    let memoryBriefDraft = $state('');
    let memoryGoalDraft = $state('');
    let memoryFocusDraft = $state('');
    let memoryConstraintDraft = $state('');
    let memoryEntryDraft = $state('');
    let handoffPrompt = $state('');
    let openFlowTitleDraft = $state('');
    let openFlowGoalDraft = $state('');
    let memoryPanel = $state('brief');
    let windowFocused = $state(true);
    let removeFocusListener: (() => void) | null = null;
    let removeBlurListener: (() => void) | null = null;

    function applyThemeVars(nextTheme = fallbackTheme) {
        const root = document.documentElement;
        for (const key of themeKeys) {
            root.style.setProperty(`--theme-${key.replaceAll('_', '-')}`, nextTheme[key]);
        }
    }

    function basename(path: string) {
        return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    }

    function compactPath(path: string, depth = 3) {
        const parts = path.split(/[\\/]/).filter(Boolean);
        if (parts.length <= depth) {
            return path;
        }

        return `.../${parts.slice(-depth).join('/')}`;
    }

    function pluralize(count: number, singular: string, plural = `${singular}s`) {
        return `${count} ${count === 1 ? singular : plural}`;
    }

    function currentWorkspace() {
        return $appState?.workspaces.find((workspace) => workspace.workspace_id === $appState?.active_workspace_id) ?? null;
    }

    function currentSurface() {
        const workspace = currentWorkspace();
        return workspace?.surfaces.find((surface) => surface.surface_id === workspace.active_surface_id) ?? null;
    }

    function activeSessionId() {
        return findActiveSessionId(currentSurface()) ?? $appState?.terminal_sessions[0]?.session_id ?? null;
    }

    function sessionsForWorkspace(workspaceId: string) {
        const workspace = $appState?.workspaces.find((entry) => entry.workspace_id === workspaceId);
        if (!workspace || !$appState) return [];

        const sessionIds = collectWorkspaceSessionIds(workspace.surfaces);

        return $appState.terminal_sessions.filter((session) => sessionIds.has(session.session_id));
    }

    function workspaceNotifications(workspaceId: string) {
        return $appState?.notifications.filter((notification) => notification.workspace_id === workspaceId) ?? [];
    }

    function activeRun() {
        return $openflowRuntime?.active_runs?.[0] ?? null;
    }

    function latestRunSummary(run: OpenFlowRunRecord) {
        return run.timeline[run.timeline.length - 1]?.message ?? 'Waiting for orchestration updates';
    }

    function sessionStateLabel(state: TerminalSessionSnapshot['state']) {
        if (state === 'ready') return 'ready';
        if (state === 'starting') return 'starting';
        if (state === 'failed') return 'failed';
        return 'exited';
    }

    function sessionStateTone(state: TerminalSessionSnapshot['state']) {
        if (state === 'ready') return 'ready';
        if (state === 'starting') return 'busy';
        if (state === 'failed') return 'danger';
        return 'muted';
    }

    function runTone(status: OpenFlowRunRecord['status']) {
        if (status === 'completed') return 'ready';
        if (status === 'failed' || status === 'cancelled') return 'danger';
        if (status === 'awaiting_approval') return 'attention';
        return 'busy';
    }

    function notificationLabel(notification: NotificationSnapshot) {
        return notification.level === 'attention' ? 'Needs input' : 'Info';
    }

    function formatNotificationTime(createdAtMs: number) {
        return new Date(createdAtMs).toLocaleTimeString([], {
            hour: '2-digit',
            minute: '2-digit'
        });
    }

    function memoryEntryCount(kind: 'pinned_context' | 'decision' | 'next_step') {
        if (!$projectMemory) {
            return 0;
        }

        if (kind === 'pinned_context') return $projectMemory.pinned_context.length;
        if (kind === 'decision') return $projectMemory.recent_decisions.length;
        return $projectMemory.next_steps.length;
    }

    function openFlowNeedsReview(run: OpenFlowRunRecord) {
        return run.current_phase === 'review' || run.status === 'awaiting_approval';
    }

    function workspaceStatus(workspace: WorkspaceSnapshot) {
        return workspace.latest_agent_state?.trim() || 'Idle';
    }

    async function handleCreateWorkspace() {
        try {
            const workspaceId = await createWorkspace();
            await activateWorkspace(workspaceId);
        } catch (error) {
            console.error('Failed to create workspace:', error);
        }
    }

    async function handleActivateWorkspace(workspaceId: string) {
        try {
            await activateWorkspace(workspaceId);
        } catch (error) {
            console.error(`Failed to activate workspace ${workspaceId}:`, error);
        }
    }

    async function handleRenameWorkspace() {
        const workspace = currentWorkspace();
        const title = renameDraft.trim();
        if (!workspace || !title) return;

        try {
            await renameWorkspace(workspace.workspace_id, title);
        } catch (error) {
            console.error(`Failed to rename workspace ${workspace.workspace_id}:`, error);
        }
    }

    async function handleCloseWorkspace(workspaceId: string) {
        try {
            await closeWorkspace(workspaceId);
        } catch (error) {
            console.error(`Failed to close workspace ${workspaceId}:`, error);
        }
    }

    async function handleCreateSession() {
        try {
            await createTerminalSession();
        } catch (error) {
            console.error('Failed to create terminal session:', error);
        }
    }

    async function handleActivateSession(sessionId: string) {
        try {
            await activateTerminalSession(sessionId);
        } catch (error) {
            console.error(`Failed to activate terminal session ${sessionId}:`, error);
        }
    }

    async function handleCloseSession(sessionId: string) {
        try {
            await closeTerminalSession(sessionId);
        } catch (error) {
            console.error(`Failed to close terminal session ${sessionId}:`, error);
        }
    }

    async function handleRestartSession(sessionId: string) {
        try {
            await restartTerminalSession(sessionId);
        } catch (error) {
            console.error(`Failed to restart terminal session ${sessionId}:`, error);
        }
    }

    async function handleActivatePane(paneId: string) {
        try {
            await activatePane(paneId);
        } catch (error) {
            console.error(`Failed to activate pane ${paneId}:`, error);
        }
    }

    async function handleSplitPane(paneId: string, direction: 'horizontal' | 'vertical') {
        try {
            await splitPane(paneId, direction);
        } catch (error) {
            console.error(`Failed to split pane ${paneId}:`, error);
        }
    }

    async function handleClosePane(paneId: string) {
        try {
            await closePane(paneId);
        } catch (error) {
            console.error(`Failed to close pane ${paneId}:`, error);
        }
    }

    async function handleResizeSplit(paneId: string, childSizes: number[]) {
        try {
            await resizeSplit(paneId, childSizes);
        } catch (error) {
            console.error(`Failed to resize split ${paneId}:`, error);
        }
    }

    async function handleCreateBrowserPane(paneId: string) {
        try {
            await createBrowserPane(paneId);
        } catch (error) {
            console.error(`Failed to create browser pane for ${paneId}:`, error);
        }
    }

    async function handleMarkWorkspaceRead(workspaceId: string) {
        try {
            await markWorkspaceNotificationsRead(workspaceId);
        } catch (error) {
            console.error(`Failed to mark notifications read for ${workspaceId}:`, error);
        }
    }

    async function handleTestNotification() {
        const sessionId = activeSessionId() ?? undefined;
        const paneId = currentSurface()?.active_pane_id;
        try {
            await notifyAttention('Agent needs your input', sessionId, paneId);
        } catch (error) {
            console.error('Failed to send attention notification:', error);
        }
    }

    async function handleNotificationSoundToggle(enabled: boolean) {
        try {
            await setNotificationSoundEnabled(enabled);
        } catch (error) {
            console.error('Failed to update notification sound setting:', error);
        }
    }

    async function handleSaveMemoryCore() {
        try {
            const constraints = memoryConstraintDraft
                .split('\n')
                .map((item) => item.trim())
                .filter(Boolean);
            await updateProjectMemory({
                project_brief: memoryBriefDraft,
                current_goal: memoryGoalDraft,
                current_focus: memoryFocusDraft,
                constraints
            });
        } catch (error) {
            console.error('Failed to save project memory core:', error);
        }
    }

    async function handleAddMemoryEntry(kind: 'pinned_context' | 'decision' | 'next_step' | 'session_summary') {
        if (!memoryEntryDraft.trim()) {
            return;
        }

        try {
            await addProjectMemoryEntry(kind, memoryEntryDraft.trim(), {
                toolName: 'codemux-ui'
            });
            memoryEntryDraft = '';
        } catch (error) {
            console.error('Failed to add project memory entry:', error);
        }
    }

    async function handleGenerateHandoff() {
        try {
            const packet = await generateProjectHandoff();
            handoffPrompt = packet.suggested_prompt;
        } catch (error) {
            console.error('Failed to generate handoff packet:', error);
        }
    }

    async function handleCreateOpenFlowRun() {
        if (!openFlowTitleDraft.trim() || !openFlowGoalDraft.trim()) {
            return;
        }

        try {
            await createOpenFlowRun({
                title: openFlowTitleDraft.trim(),
                goal: openFlowGoalDraft.trim()
            });
            openFlowTitleDraft = '';
            openFlowGoalDraft = '';
        } catch (error) {
            console.error('Failed to create OpenFlow run:', error);
        }
    }

    async function handleAdvanceOpenFlowRun(runId: string) {
        try {
            await advanceOpenFlowRunPhase(runId);
        } catch (error) {
            console.error(`Failed to advance OpenFlow run ${runId}:`, error);
        }
    }

    async function handleRetryOpenFlowRun(runId: string) {
        try {
            await retryOpenFlowRun(runId);
        } catch (error) {
            console.error(`Failed to retry OpenFlow run ${runId}:`, error);
        }
    }

    async function handleRunOpenFlowLoop(runId: string) {
        try {
            await runOpenFlowAutonomousLoop(runId);
        } catch (error) {
            console.error(`Failed to run OpenFlow loop for ${runId}:`, error);
        }
    }

    async function handleApproveOpenFlowRun(runId: string) {
        try {
            await applyOpenFlowReviewResult(runId, 95, true, null);
        } catch (error) {
            console.error(`Failed to approve OpenFlow run ${runId}:`, error);
        }
    }

    async function handleRejectOpenFlowRun(runId: string) {
        try {
            await applyOpenFlowReviewResult(runId, 58, false, 'Reviewer requested additional fixes');
        } catch (error) {
            console.error(`Failed to reject OpenFlow run ${runId}:`, error);
        }
    }

    async function handlePauseOpenFlowRun(runId: string) {
        try {
            await stopOpenFlowRun(runId, 'awaiting_approval', 'Paused for user approval');
        } catch (error) {
            console.error(`Failed to pause OpenFlow run ${runId}:`, error);
        }
    }

    async function handleCancelOpenFlowRun(runId: string) {
        try {
            await stopOpenFlowRun(runId, 'cancelled', 'Cancelled by user');
        } catch (error) {
            console.error(`Failed to cancel OpenFlow run ${runId}:`, error);
        }
    }

    function handleWindowKeydown(event: KeyboardEvent) {
        if (!(event.metaKey || event.ctrlKey)) return;

        if (event.key === ']') {
            event.preventDefault();
            void cycleWorkspace(1);
            return;
        }

        if (event.key === '[') {
            event.preventDefault();
            void cycleWorkspace(-1);
            return;
        }

        if (event.shiftKey && event.key.toLowerCase() === 'j') {
            event.preventDefault();
            void cyclePane(1);
            return;
        }

        if (event.shiftKey && event.key.toLowerCase() === 'k') {
            event.preventDefault();
            void cyclePane(-1);
            return;
        }

        if (event.key.toLowerCase() === 'l') {
            event.preventDefault();
            void cyclePane(1);
            return;
        }

        if (event.key.toLowerCase() === 'h') {
            event.preventDefault();
            void cyclePane(-1);
            return;
        }

        if (event.altKey && event.key === 'ArrowLeft') {
            event.preventDefault();
            void resizeActivePane(-0.05);
            return;
        }

        if (event.altKey && event.key === 'ArrowRight') {
            event.preventDefault();
            void resizeActivePane(0.05);
            return;
        }

        if (event.altKey && event.key === 'ArrowUp') {
            event.preventDefault();
            void resizeActivePane(-0.05);
            return;
        }

        if (event.altKey && event.key === 'ArrowDown') {
            event.preventDefault();
            void resizeActivePane(0.05);
            return;
        }

        if (event.altKey && event.key.toLowerCase() === 'h') {
            event.preventDefault();
            void resizeActivePane(-0.05);
            return;
        }

        if (event.altKey && event.key.toLowerCase() === 'l') {
            event.preventDefault();
            void resizeActivePane(0.05);
            return;
        }

        if (event.altKey && event.key.toLowerCase() === 'k') {
            event.preventDefault();
            void resizeActivePane(-0.05);
            return;
        }

        if (event.altKey && event.key.toLowerCase() === 'j') {
            event.preventDefault();
            void resizeActivePane(0.05);
        }
    }

    onMount(() => {
        applyThemeVars(get(theme) ?? fallbackTheme);
        initTheme();
        initAppState();
        initProjectMemory();
        initOpenFlowRuntime();

        const themeUnsubscribe = theme.subscribe((nextTheme) => {
            applyThemeVars(nextTheme ?? fallbackTheme);
        });

        const appStateUnsubscribe = appState.subscribe((snapshot) => {
            renameDraft = snapshot?.workspaces.find((workspace) => workspace.workspace_id === snapshot.active_workspace_id)?.title ?? '';
        });
        const projectMemoryUnsubscribe = projectMemory.subscribe((snapshot) => {
            memoryBriefDraft = snapshot?.project_brief ?? '';
            memoryGoalDraft = snapshot?.current_goal ?? '';
            memoryFocusDraft = snapshot?.current_focus ?? '';
            memoryConstraintDraft = snapshot?.constraints.join('\n') ?? '';
        });

        window.addEventListener('keydown', handleWindowKeydown);
        const onFocus = () => {
            windowFocused = true;
            const workspace = currentWorkspace();
            if (workspace) {
                void handleMarkWorkspaceRead(workspace.workspace_id);
            }
        };
        const onBlur = () => {
            windowFocused = false;
        };
        window.addEventListener('focus', onFocus);
        window.addEventListener('blur', onBlur);
        removeFocusListener = () => window.removeEventListener('focus', onFocus);
        removeBlurListener = () => window.removeEventListener('blur', onBlur);

        return () => {
            themeUnsubscribe();
            appStateUnsubscribe();
            projectMemoryUnsubscribe();
            window.removeEventListener('keydown', handleWindowKeydown);
            removeFocusListener?.();
            removeBlurListener?.();
        };
    });
</script>

<main class="app-shell">
    {#if $appState && $appState.workspaces.length > 0}
        <header class="app-header">
            <div class="brand-block">
                <div class="brand-mark">
                    <span class="brand-dot"></span>
                    <span>Codemux</span>
                </div>
                <div class="workspace-heading">
                    <h1>{currentWorkspace()?.title ?? 'Workspace'}</h1>
                    <p>{compactPath(currentWorkspace()?.cwd ?? '')}</p>
                </div>
            </div>

            <div class="status-strip">
                <span class="meta-pill">{pluralize($appState.workspaces.length, 'workspace')}</span>
                <span class="meta-pill">{pluralize($appState.terminal_sessions.length, 'terminal')}</span>
                <span class="meta-pill">{currentSurface() ? 'live split surface' : 'no active surface'}</span>
            </div>
        </header>

        <section class="workspace-shell">
            <aside class="workspace-sidebar">
                <section class="sidebar-card workspace-rail">
                    <div class="section-header">
                        <div>
                            <p class="section-kicker">Workspaces</p>
                            <h2>Workspace rail</h2>
                        </div>
                        <button class="quiet-button" type="button" onclick={handleCreateWorkspace}>New</button>
                    </div>

                    <div class="workspace-list">
                        {#each $appState.workspaces as workspace}
                            <div
                                class="workspace-card"
                                class:active={workspace.workspace_id === $appState.active_workspace_id}
                                role="button"
                                tabindex="0"
                                onclick={() => handleActivateWorkspace(workspace.workspace_id)}
                                onkeydown={(event) => {
                                    if (event.key === 'Enter' || event.key === ' ') {
                                        event.preventDefault();
                                        void handleActivateWorkspace(workspace.workspace_id);
                                    }
                                }}
                            >
                                <div class="workspace-card-top">
                                    <div class="workspace-title-block">
                                        <span class={`status-dot ${workspace.notification_count > 0 ? 'attention' : 'ready'}`}></span>
                                        <div>
                                            <strong>{workspace.title}</strong>
                                            <p>{basename(workspace.cwd)}</p>
                                        </div>
                                    </div>
                                    {#if workspace.workspace_id === $appState.active_workspace_id}
                                        <span class="status-pill ready">active</span>
                                    {/if}
                                </div>

                                <p class="workspace-path">{compactPath(workspace.cwd)}</p>
                                <p class="workspace-state">{workspaceStatus(workspace)}</p>

                                <div class="workspace-meta">
                                    <span class="soft-tag">{pluralize(sessionsForWorkspace(workspace.workspace_id).length, 'shell')}</span>
                                    <span class="soft-tag">{workspace.git_branch ?? 'no branch yet'}</span>
                                    {#if workspace.notification_count > 0}
                                        <span class="soft-tag attention">{pluralize(workspace.notification_count, 'alert')}</span>
                                    {/if}
                                </div>

                                <div class="workspace-actions">
                                    {#if workspace.notification_count > 0}
                                        <button class="quiet-button" type="button" onclick={(event) => {
                                            event.stopPropagation();
                                            void handleMarkWorkspaceRead(workspace.workspace_id);
                                        }}>Mark read</button>
                                    {/if}
                                    <button class="quiet-button" type="button" onclick={(event) => {
                                        event.stopPropagation();
                                        void handleCloseWorkspace(workspace.workspace_id);
                                    }}>Close</button>
                                </div>
                            </div>
                        {/each}
                    </div>
                </section>

                <section class="sidebar-card context-card">
                    <div class="section-header">
                        <div>
                            <p class="section-kicker">Active context</p>
                            <h2>{currentWorkspace()?.title ?? 'Workspace'}</h2>
                        </div>
                        <span class={`status-pill ${windowFocused ? 'ready' : 'muted'}`}>{windowFocused ? 'focused' : 'background'}</span>
                    </div>

                    {#if activeRun()}
                        <div class="run-banner">
                            <div class="run-banner-main">
                                <span class={`status-dot ${runTone(activeRun()!.status)}`}></span>
                                <div>
                                    <strong>{activeRun()!.title}</strong>
                                    <p>{latestRunSummary(activeRun()!)}</p>
                                </div>
                            </div>
                            <span class={`status-pill ${runTone(activeRun()!.status)}`}>{activeRun()!.current_phase}</span>
                        </div>
                    {/if}

                    <label class="field">
                        <span>Workspace name</span>
                        <input bind:value={renameDraft} placeholder="Workspace name" />
                    </label>

                    <div class="action-row">
                        <button class="primary-button" type="button" onclick={handleRenameWorkspace}>Save name</button>
                        <button class="quiet-button" type="button" onclick={handleTestNotification}>Test attention</button>
                    </div>

                    <div class="context-footer">
                        <label class="toggle-row compact">
                            <span>Notification sound</span>
                            <input
                                type="checkbox"
                                checked={$appState.config.notification_sound_enabled}
                                onchange={(event) => handleNotificationSoundToggle((event.currentTarget as HTMLInputElement).checked)}
                            />
                        </label>
                        <p class="hint">Use `Ctrl/Cmd + [` / `]`, `Ctrl/Cmd + H/L`, `Alt + H/J/K/L`.</p>
                    </div>

                    <div class="subpanel">
                        <div class="subpanel-header">
                            <h3>Sessions</h3>
                            <button class="quiet-button" type="button" onclick={handleCreateSession}>New shell</button>
                        </div>

                        <div class="session-list">
                            {#each sessionsForWorkspace(currentWorkspace()?.workspace_id ?? '') as session}
                                <div
                                    class="session-card"
                                    class:active={session.session_id === activeSessionId()}
                                    role="button"
                                    tabindex="0"
                                    onclick={() => handleActivateSession(session.session_id)}
                                    onkeydown={(event) => {
                                        if (event.key === 'Enter' || event.key === ' ') {
                                            event.preventDefault();
                                            void handleActivateSession(session.session_id);
                                        }
                                    }}
                                >
                                    <div class="session-card-top">
                                        <div>
                                            <strong>{session.title}</strong>
                                            <p>{basename(session.cwd)}</p>
                                        </div>
                                        <span class={`status-pill ${sessionStateTone(session.state)}`}>{sessionStateLabel(session.state)}</span>
                                    </div>

                                    <div class="session-meta">
                                        <span>{basename(session.shell ?? 'shell')}</span>
                                        <span>{session.cols}x{session.rows}</span>
                                    </div>

                                    <div class="session-actions">
                                        <button class="quiet-button" type="button" onclick={(event) => {
                                            event.stopPropagation();
                                            void handleRestartSession(session.session_id);
                                        }}>Restart</button>
                                        <button class="quiet-button" type="button" onclick={(event) => {
                                            event.stopPropagation();
                                            void handleCloseSession(session.session_id);
                                        }}>Close</button>
                                    </div>
                                </div>
                            {/each}
                        </div>
                    </div>

                    <div class="subpanel">
                        <div class="subpanel-header">
                            <h3>Notifications</h3>
                            {#if workspaceNotifications(currentWorkspace()?.workspace_id ?? '').length > 0}
                                <button class="quiet-button" type="button" onclick={() => handleMarkWorkspaceRead(currentWorkspace()?.workspace_id ?? '')}>Mark read</button>
                            {/if}
                        </div>

                        <div class="notification-feed">
                            {#if workspaceNotifications(currentWorkspace()?.workspace_id ?? '').length > 0}
                                {#each workspaceNotifications(currentWorkspace()?.workspace_id ?? '') as notification}
                                    <div class={`notification-item ${notification.read ? 'read' : 'unread'}`}>
                                        <div class="notification-top">
                                            <span class={`status-pill ${notification.level === 'attention' ? 'attention' : 'muted'}`}>{notificationLabel(notification)}</span>
                                            <span class="notification-time">{formatNotificationTime(notification.created_at_ms)}</span>
                                        </div>
                                        <p>{notification.message}</p>
                                    </div>
                                {/each}
                            {:else}
                                <div class="empty-inline-card">
                                    <strong>Nothing is waiting on you.</strong>
                                    <p>Attention pings and agent state changes will show up here.</p>
                                </div>
                            {/if}
                        </div>
                    </div>
                </section>

                <section class="sidebar-card openflow-card">
                    <div class="section-header">
                        <div>
                            <p class="section-kicker">OpenFlow</p>
                            <h2>Run composer</h2>
                        </div>
                        <span class="meta-pill">{pluralize($openflowRuntime?.active_runs?.length ?? 0, 'run')}</span>
                    </div>

                    <div class="composer-card">
                        <label class="field compact">
                            <span>Run title</span>
                            <input bind:value={openFlowTitleDraft} placeholder="Barbershop booking build" />
                        </label>
                        <label class="field compact">
                            <span>Main goal</span>
                            <textarea bind:value={openFlowGoalDraft} rows="3" placeholder="Describe what the orchestrator should build"></textarea>
                        </label>
                        <button class="primary-button wide-button" type="button" onclick={handleCreateOpenFlowRun}>Start run</button>
                    </div>

                    <div class="openflow-run-list">
                        {#if $openflowRuntime?.active_runs?.length}
                            {#each $openflowRuntime.active_runs as run}
                                <article class="openflow-run-card">
                                    <div class="openflow-run-top">
                                        <div>
                                            <strong>{run.title}</strong>
                                            <p>{run.goal}</p>
                                        </div>
                                        <span class={`status-pill ${runTone(run.status)}`}>{run.status}</span>
                                    </div>

                                    <div class="openflow-run-meta">
                                        <span>{run.current_phase}</span>
                                        <span>{pluralize(run.task_graph.length, 'task')}</span>
                                        <span>{run.reviewer_score !== null ? `${run.reviewer_score}/100` : 'score pending'}</span>
                                    </div>

                                    <p class="run-latest">{latestRunSummary(run)}</p>

                                    <div class="openflow-run-actions">
                                        <button class="quiet-button" type="button" onclick={() => handleRunOpenFlowLoop(run.run_id)}>Loop</button>
                                        <button class="quiet-button" type="button" onclick={() => handleAdvanceOpenFlowRun(run.run_id)}>Advance</button>
                                        <button class="quiet-button" type="button" onclick={() => handleRetryOpenFlowRun(run.run_id)}>Retry</button>
                                        {#if run.status !== 'completed' && run.status !== 'cancelled'}
                                            <button class="quiet-button" type="button" onclick={() => handlePauseOpenFlowRun(run.run_id)}>Pause</button>
                                            <button class="quiet-button" type="button" onclick={() => handleCancelOpenFlowRun(run.run_id)}>Cancel</button>
                                        {/if}
                                        {#if openFlowNeedsReview(run)}
                                            <button class="primary-button" type="button" onclick={() => handleApproveOpenFlowRun(run.run_id)}>Approve</button>
                                            <button class="quiet-button" type="button" onclick={() => handleRejectOpenFlowRun(run.run_id)}>Request fixes</button>
                                        {/if}
                                    </div>
                                </article>
                            {/each}
                        {:else}
                            <div class="empty-inline-card">
                                <strong>No runs yet.</strong>
                                <p>Start a run when you want orchestration layered on top of the workspace.</p>
                            </div>
                        {/if}
                    </div>
                </section>

                <section class="sidebar-card memory-card">
                    <div class="section-header">
                        <div>
                            <p class="section-kicker">Project memory</p>
                            <h2>Shared context</h2>
                        </div>
                        <span class="meta-pill">{$projectMemory?.project_name ?? basename(currentWorkspace()?.cwd ?? 'project')}</span>
                    </div>

                    <div class="memory-stats">
                        <span class="soft-tag">{memoryEntryCount('pinned_context')} pinned</span>
                        <span class="soft-tag">{memoryEntryCount('decision')} decisions</span>
                        <span class="soft-tag">{memoryEntryCount('next_step')} next steps</span>
                    </div>

                    <div class="tab-row">
                        <button class:active={memoryPanel === 'brief'} type="button" onclick={() => (memoryPanel = 'brief')}>Brief</button>
                        <button class:active={memoryPanel === 'goal'} type="button" onclick={() => (memoryPanel = 'goal')}>Goal</button>
                        <button class:active={memoryPanel === 'notes'} type="button" onclick={() => (memoryPanel = 'notes')}>Notes</button>
                    </div>

                    {#if memoryPanel === 'brief'}
                        <div class="memory-panel-content">
                            <label class="field compact">
                                <span>Project brief</span>
                                <textarea bind:value={memoryBriefDraft} rows="4" placeholder="What is this project trying to become?"></textarea>
                            </label>

                            <label class="field compact">
                                <span>Constraints</span>
                                <textarea bind:value={memoryConstraintDraft} rows="4" placeholder="One per line"></textarea>
                            </label>

                            <div class="action-row stacked-on-mobile">
                                <button class="primary-button" type="button" onclick={handleSaveMemoryCore}>Save memory</button>
                                <p class="hint">Stored locally in `.codemux/project-memory.json` for future sessions and tool handoffs.</p>
                            </div>
                        </div>
                    {:else if memoryPanel === 'goal'}
                        <div class="memory-panel-content">
                            <label class="field compact">
                                <span>Current goal</span>
                                <textarea bind:value={memoryGoalDraft} rows="3" placeholder="What are we trying to accomplish right now?"></textarea>
                            </label>

                            <label class="field compact">
                                <span>Current focus</span>
                                <textarea bind:value={memoryFocusDraft} rows="3" placeholder="What should the next session pick up first?"></textarea>
                            </label>

                            <div class="action-row stacked-on-mobile">
                                <button class="primary-button" type="button" onclick={handleSaveMemoryCore}>Save memory</button>
                                <p class="hint">This is the compact project memory that future agents can use instead of replaying long chats.</p>
                            </div>
                        </div>
                    {:else}
                        <div class="memory-panel-content">
                            <label class="field compact">
                                <span>Quick note</span>
                                <textarea bind:value={memoryEntryDraft} rows="4" placeholder="Pin context, record a decision, or capture the next step."></textarea>
                            </label>

                            <div class="memory-actions">
                                <button class="quiet-button" type="button" onclick={() => handleAddMemoryEntry('pinned_context')}>Pin</button>
                                <button class="quiet-button" type="button" onclick={() => handleAddMemoryEntry('decision')}>Decision</button>
                                <button class="quiet-button" type="button" onclick={() => handleAddMemoryEntry('next_step')}>Next</button>
                                <button class="quiet-button" type="button" onclick={() => handleAddMemoryEntry('session_summary')}>Session</button>
                            </div>

                            <div class="action-row stacked-on-mobile">
                                <button class="primary-button" type="button" onclick={handleGenerateHandoff}>Generate handoff</button>
                                <p class="hint">Use handoff when you want a fresh agent to continue with the right context.</p>
                            </div>

                            {#if handoffPrompt}
                                <label class="field compact handoff-field">
                                    <span>Generated handoff prompt</span>
                                    <textarea class="handoff-output" readonly rows="9" value={handoffPrompt}></textarea>
                                </label>
                            {/if}
                        </div>
                    {/if}
                </section>
            </aside>

            <section class="workspace-main">
                <div class="workspace-stage">
                    {#if currentSurface()}
                        <PaneNode
                            node={currentSurface()!.root}
                            activePaneId={currentSurface()!.active_pane_id}
                            on:activate={(event) => handleActivatePane(event.detail.paneId)}
                            on:split={(event) => handleSplitPane(event.detail.paneId, event.detail.direction)}
                            on:close={(event) => handleClosePane(event.detail.paneId)}
                            on:resize={(event) => handleResizeSplit(event.detail.paneId, event.detail.childSizes)}
                            on:browser={(event) => handleCreateBrowserPane(event.detail.paneId)}
                        />
                    {:else}
                        <div class="empty-card">
                            <h2>No active surface</h2>
                            <p>Create a workspace or shell to start shaping the session.</p>
                            <button class="primary-button" type="button" onclick={handleCreateSession}>Create shell</button>
                        </div>
                    {/if}
                </div>
            </section>
        </section>
    {:else}
        <section class="loading-shell">
            <div class="loading-card">
                <h1>Loading Codemux</h1>
                <p>Waiting for the workspace state to arrive from the backend.</p>
            </div>
        </section>
    {/if}
</main>

<style>
    :global(html),
    :global(body) {
        --ui-accent: var(--theme-accent, #7aa2f7);
        --ui-accent-soft: color-mix(in srgb, var(--ui-accent) 18%, transparent);
        --ui-app-bg: color-mix(in srgb, var(--theme-background, #1a1b26) 96%, #05070d 4%);
        --ui-sidebar-bg: color-mix(in srgb, var(--theme-background, #1a1b26) 92%, #0c1220 8%);
        --ui-surface: color-mix(in srgb, var(--theme-background, #1a1b26) 84%, white 16%);
        --ui-surface-strong: color-mix(in srgb, var(--theme-background, #1a1b26) 80%, white 20%);
        --ui-pane-bg: color-mix(in srgb, var(--theme-background, #1a1b26) 94%, #0f172a 6%);
        --ui-pane-bg-strong: color-mix(in srgb, var(--theme-background, #1a1b26) 90%, #111827 10%);
        --ui-border-soft: color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        --ui-border-strong: color-mix(in srgb, var(--theme-foreground, #c0caf5) 18%, transparent);
        --ui-text-primary: color-mix(in srgb, var(--theme-foreground, #c0caf5) 90%, white 10%);
        --ui-text-secondary: color-mix(in srgb, var(--theme-foreground, #c0caf5) 66%, white 34%);
        --ui-text-muted: color-mix(in srgb, var(--theme-foreground, #c0caf5) 48%, transparent);
        --ui-attention: color-mix(in srgb, var(--theme-color11, #e0af68) 74%, white 26%);
        --ui-attention-soft: color-mix(in srgb, var(--theme-color11, #e0af68) 18%, transparent);
        --ui-success: color-mix(in srgb, var(--theme-color10, #9ece6a) 74%, white 26%);
        --ui-danger: color-mix(in srgb, var(--theme-color1, #f7768e) 74%, white 26%);
        --ui-radius-sm: 8px;
        --ui-radius-md: 10px;
        --ui-radius-lg: 12px;
        width: 100%;
        height: 100%;
        margin: 0;
        padding: 0;
        background: var(--ui-app-bg);
        color: var(--ui-text-primary);
        font-family: 'IBM Plex Mono', 'JetBrains Mono', 'SFMono-Regular', Menlo, Consolas, monospace;
        font-size: 13px;
        overflow: hidden;
    }

    .app-shell {
        display: flex;
        flex-direction: column;
        width: 100vw;
        height: 100dvh;
        min-width: 0;
        min-height: 0;
    }

    .app-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 24px;
        padding: 12px 18px;
        border-bottom: 1px solid var(--ui-border-soft);
        background: var(--ui-sidebar-bg);
    }

    .brand-block {
        display: flex;
        align-items: center;
        gap: 18px;
        min-width: 0;
    }

    .brand-mark {
        display: inline-flex;
        align-items: center;
        gap: 10px;
        padding: 7px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: 10px;
        background: var(--ui-surface);
        color: var(--ui-text-primary);
        font-size: 0.8rem;
        font-weight: 600;
        letter-spacing: 0.03em;
    }

    .brand-dot {
        width: 8px;
        height: 8px;
        border-radius: 2px;
        background: var(--ui-accent);
    }

    .workspace-heading {
        min-width: 0;
    }

    .workspace-heading h1,
    .loading-card h1,
    .empty-card h2 {
        margin: 0;
        font-size: 1rem;
        font-weight: 600;
        letter-spacing: 0;
    }

    .workspace-heading p,
    .loading-card p,
    .empty-card p,
    .workspace-path,
    .workspace-state,
    .run-banner p,
    .hint,
    .notification-item p,
    .openflow-run-top p,
    .run-latest,
    .empty-inline-card p {
        margin: 0;
        color: var(--ui-text-secondary);
        line-height: 1.45;
    }

    .status-strip {
        display: flex;
        flex-wrap: wrap;
        justify-content: flex-end;
        gap: 10px;
    }

    .meta-pill,
    .soft-tag,
    .status-pill {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        gap: 8px;
        padding: 6px 10px;
        border-radius: 8px;
        border: 1px solid var(--ui-border-soft);
        background: var(--ui-surface);
        color: var(--ui-text-secondary);
        font-size: 0.72rem;
        white-space: nowrap;
    }

    .status-pill.ready,
    .status-dot.ready {
        color: var(--ui-success);
    }

    .status-pill.busy,
    .status-dot.busy {
        color: var(--ui-accent);
    }

    .status-pill.attention,
    .status-dot.attention {
        color: var(--ui-attention);
    }

    .status-pill.danger,
    .status-dot.danger {
        color: var(--ui-danger);
    }

    .status-pill.ready {
        background: color-mix(in srgb, var(--ui-success) 12%, transparent);
        border-color: color-mix(in srgb, var(--ui-success) 22%, transparent);
    }

    .status-pill.busy {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        border-color: color-mix(in srgb, var(--ui-accent) 22%, transparent);
        color: var(--ui-accent);
    }

    .status-pill.attention {
        background: var(--ui-attention-soft);
        border-color: color-mix(in srgb, var(--ui-attention) 22%, transparent);
        color: var(--ui-attention);
    }

    .status-pill.danger {
        background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
        border-color: color-mix(in srgb, var(--ui-danger) 22%, transparent);
        color: var(--ui-danger);
    }

    .status-pill.muted {
        color: var(--ui-text-muted);
    }

    .soft-tag.attention {
        color: var(--ui-attention);
        border-color: color-mix(in srgb, var(--ui-attention) 18%, transparent);
        background: var(--ui-attention-soft);
    }

    .status-dot {
        width: 8px;
        height: 8px;
        border-radius: 2px;
        background: currentColor;
        flex: 0 0 auto;
    }

    .workspace-shell {
        display: grid;
        grid-template-columns: 320px minmax(0, 1fr);
        flex: 1;
        min-height: 0;
    }

    .workspace-sidebar {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 12px;
        border-right: 1px solid var(--ui-border-soft);
        background: var(--ui-sidebar-bg);
        overflow-y: auto;
        overflow-x: hidden;
    }

    .sidebar-card,
    .workspace-stage,
    .loading-card,
    .empty-card {
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-lg);
        background: var(--ui-surface);
    }

    .sidebar-card {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 12px;
        min-width: 0;
    }

    .section-header,
    .subpanel-header,
    .workspace-card-top,
    .session-card-top,
    .openflow-run-top,
    .notification-top,
    .action-row,
    .workspace-actions,
    .session-actions,
    .memory-stats,
    .memory-actions,
    .run-banner,
    .run-banner-main {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .section-header,
    .subpanel-header,
    .workspace-card-top,
    .session-card-top,
    .openflow-run-top,
    .notification-top,
    .action-row,
    .workspace-actions {
        justify-content: space-between;
    }

    .section-kicker {
        margin: 0 0 4px;
        color: var(--ui-accent);
        font-size: 0.74rem;
        font-weight: 600;
        letter-spacing: 0.04em;
        text-transform: uppercase;
    }

    .section-header h2,
    .subpanel-header h3 {
        margin: 0;
        font-size: 0.98rem;
        font-weight: 600;
    }

    button,
    input,
    textarea {
        font: inherit;
    }

    .primary-button,
    .quiet-button,
    .tab-row button {
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        background: var(--ui-surface-strong);
        color: var(--ui-text-primary);
        padding: 8px 10px;
        cursor: pointer;
        transition:
            background 100ms ease-out,
            border-color 100ms ease-out,
            color 100ms ease-out,
            opacity 100ms ease-out;
    }

    .primary-button:hover,
    .quiet-button:hover,
    .tab-row button:hover,
    .workspace-card:hover,
    .session-card:hover {
        border-color: var(--ui-border-strong);
        background: color-mix(in srgb, var(--ui-surface-strong) 92%, transparent);
    }

    .primary-button {
        background: color-mix(in srgb, var(--ui-accent) 12%, var(--ui-surface-strong) 88%);
        border-color: color-mix(in srgb, var(--ui-accent) 26%, transparent);
    }

    .quiet-button {
        color: var(--ui-text-secondary);
    }

    .wide-button {
        width: 100%;
        justify-content: center;
    }

    .workspace-list,
    .session-list,
    .notification-feed,
    .openflow-run-list,
    .memory-panel-content {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .workspace-card,
    .session-card,
    .openflow-run-card,
    .empty-inline-card,
    .composer-card,
    .subpanel,
    .run-banner,
    .notification-item {
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-md);
        background: var(--ui-pane-bg-strong);
    }

    .workspace-card,
    .session-card,
    .openflow-run-card,
    .empty-inline-card,
    .composer-card,
    .subpanel,
    .notification-item {
        padding: 14px;
    }

    .workspace-card,
    .session-card {
        display: flex;
        flex-direction: column;
        gap: 10px;
        cursor: pointer;
    }

    .workspace-card.active,
    .session-card.active {
        border-color: color-mix(in srgb, var(--ui-accent) 28%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 8%, var(--ui-pane-bg-strong) 92%);
    }

    .workspace-title-block {
        display: flex;
        align-items: center;
        gap: 12px;
        min-width: 0;
    }

    .workspace-title-block strong,
    .session-card strong,
    .openflow-run-card strong,
    .empty-inline-card strong {
        display: block;
        font-size: 0.96rem;
        font-weight: 600;
    }

    .workspace-title-block p,
    .session-card p,
    .openflow-run-card p,
    .notification-item p {
        margin: 2px 0 0;
    }

    .workspace-meta,
    .session-meta,
    .openflow-run-meta,
    .memory-stats,
    .memory-actions,
    .workspace-actions,
    .session-actions,
    .openflow-run-actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }

    .workspace-actions,
    .session-actions,
    .openflow-run-actions {
        margin-top: 2px;
    }

    .workspace-state,
    .notification-time,
    .session-meta,
    .openflow-run-meta,
    .hint {
        color: var(--ui-text-muted);
        font-size: 0.78rem;
    }

    .field {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .field span {
        color: var(--ui-text-secondary);
        font-size: 0.8rem;
        font-weight: 600;
    }

    .field.compact span {
        font-size: 0.78rem;
    }

    input,
    textarea {
        width: 100%;
        max-width: 100%;
        box-sizing: border-box;
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        background: var(--ui-pane-bg);
        color: var(--ui-text-primary);
        padding: 10px;
        resize: vertical;
        transition: border-color 100ms ease-out, background 100ms ease-out;
    }

    input:focus,
    textarea:focus {
        outline: none;
        border-color: color-mix(in srgb, var(--ui-accent) 32%, transparent);
    }

    .context-footer {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .toggle-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
    }

    .toggle-row.compact {
        padding: 8px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        background: var(--ui-pane-bg);
        color: var(--ui-text-secondary);
    }

    .tab-row {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }

    .tab-row button.active {
        border-color: color-mix(in srgb, var(--ui-accent) 26%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 14%, transparent);
        color: var(--ui-text-primary);
    }

    .handoff-output {
        min-height: 190px;
    }

    .workspace-main {
        min-width: 0;
        min-height: 0;
        padding: 12px;
    }

    .workspace-stage {
        position: relative;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        background: var(--ui-pane-bg);
    }

    .loading-shell,
    .empty-card {
        display: flex;
        align-items: center;
        justify-content: center;
    }

    .loading-shell {
        flex: 1;
        padding: 24px;
    }

    .loading-card,
    .empty-card {
        flex-direction: column;
        gap: 12px;
        width: min(460px, 100%);
        padding: 18px;
        text-align: center;
    }

    .empty-card {
        height: 100%;
        width: 100%;
        border-style: dashed;
        background: var(--ui-pane-bg);
    }

    .run-banner {
        justify-content: space-between;
        padding: 14px;
    }

    .run-banner-main {
        align-items: flex-start;
    }

    .run-banner-main strong {
        display: block;
        margin-bottom: 2px;
    }

    .notification-item.read {
        opacity: 0.68;
    }

    .notification-item.unread {
        border-color: color-mix(in srgb, var(--ui-attention) 28%, transparent);
        background: color-mix(in srgb, var(--ui-attention) 6%, var(--ui-pane-bg-strong) 94%);
    }

    .empty-inline-card {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .openflow-run-top {
        align-items: flex-start;
    }

    .run-latest {
        font-size: 0.84rem;
    }

    .stacked-on-mobile {
        align-items: flex-start;
    }

    @media (max-width: 1180px) {
        .workspace-shell {
            grid-template-columns: 320px minmax(0, 1fr);
        }

        .app-header {
            padding-inline: 18px;
        }

        .workspace-main {
            padding: 14px;
        }
    }

    @media (max-width: 920px) {
        .workspace-shell {
            grid-template-columns: 1fr;
        }

        .workspace-sidebar {
            max-height: 48vh;
            border-right: 0;
            border-bottom: 1px solid var(--ui-border-soft);
        }

        .app-header {
            flex-direction: column;
            align-items: flex-start;
        }

        .status-strip {
            justify-content: flex-start;
        }

        .stacked-on-mobile {
            flex-direction: column;
        }
    }
</style>
