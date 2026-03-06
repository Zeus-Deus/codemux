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
        retryOpenFlowRun,
        splitPane
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
    let windowFocused = $state(true);
    let removeFocusListener: (() => void) | null = null;
    let removeBlurListener: (() => void) | null = null;

    function applyThemeVars(nextTheme = fallbackTheme) {
        const root = document.documentElement;
        for (const key of themeKeys) {
            root.style.setProperty(`--theme-${key.replaceAll('_', '-')}`, nextTheme[key]);
        }
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

    function workspaceNotifications(workspaceId: string) {
        return $appState?.notifications.filter((notification) => notification.workspace_id === workspaceId) ?? [];
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

<main class="app-container">
    {#if $appState && $appState.workspaces.length > 0}
        <header class="app-header">
            <div>
                <span class="eyebrow">Codemux Prototype</span>
                <h1>{currentWorkspace()?.title ?? 'Workspace'}</h1>
            </div>
            <div class="status-strip">
                <span>{$appState.workspaces.length} workspace</span>
                <span>{$appState.terminal_sessions.length} terminal</span>
                <span>{currentSurface() ? 'split layout active' : 'no surface'}</span>
            </div>
        </header>

        <section class="workspace-shell">
            <aside class="workspace-sidebar">
                <div class="panel-card workspace-panel">
                    <div class="panel-card-header">
                        <h2>Workspaces</h2>
                        <button type="button" onclick={handleCreateWorkspace}>New</button>
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
                                <strong>{workspace.title}</strong>
                                <span>{workspace.cwd}</span>
                                <small>{workspace.latest_agent_state ?? 'idle'}</small>
                                <div class="workspace-meta">
                                    <span>{sessionsForWorkspace(workspace.workspace_id).length} session</span>
                                    {#if workspace.notification_count > 0}
                                        <span class="notification-badge">{workspace.notification_count} alert</span>
                                    {/if}
                                    <button type="button" onclick={(event) => {
                                        event.stopPropagation();
                                        void handleMarkWorkspaceRead(workspace.workspace_id);
                                    }}>Read</button>
                                    <button type="button" onclick={(event) => {
                                        event.stopPropagation();
                                        void handleCloseWorkspace(workspace.workspace_id);
                                    }}>Close</button>
                                </div>
                            </div>
                        {/each}
                    </div>
                </div>

                <div class="panel-card details-panel">
                    <h2>Workspace Details</h2>
                    <label>
                        <span>Name</span>
                        <input bind:value={renameDraft} placeholder="Workspace name" />
                    </label>
                    <button type="button" onclick={handleRenameWorkspace}>Save Name</button>
                    <button type="button" onclick={handleTestNotification}>Test Attention</button>
                    <label class="toggle-row">
                        <span>Notification sound</span>
                        <input
                            type="checkbox"
                            checked={$appState.config.notification_sound_enabled}
                            onchange={(event) => handleNotificationSoundToggle((event.currentTarget as HTMLInputElement).checked)}
                        />
                    </label>
                    <small class="hint">Window focus: {windowFocused ? 'focused' : 'unfocused'}</small>
                    <small class="hint">Use `Ctrl/Cmd + [` and `Ctrl/Cmd + ]` for workspaces, `Ctrl/Cmd + Shift + J/K` for pane focus.</small>
                </div>

                <div class="panel-card memory-panel">
                    <div class="panel-card-header">
                        <h2>Project Memory</h2>
                        <button type="button" onclick={handleGenerateHandoff}>Handoff</button>
                    </div>

                    <label>
                        <span>Project brief</span>
                        <textarea bind:value={memoryBriefDraft} rows="3"></textarea>
                    </label>
                    <label>
                        <span>Current goal</span>
                        <textarea bind:value={memoryGoalDraft} rows="2"></textarea>
                    </label>
                    <label>
                        <span>Current focus</span>
                        <textarea bind:value={memoryFocusDraft} rows="2"></textarea>
                    </label>
                    <label>
                        <span>Constraints</span>
                        <textarea bind:value={memoryConstraintDraft} rows="3" placeholder="One per line"></textarea>
                    </label>
                    <button type="button" onclick={handleSaveMemoryCore}>Save Memory</button>

                    <label>
                        <span>Quick memory entry</span>
                        <textarea bind:value={memoryEntryDraft} rows="3" placeholder="Add a decision, next step, or pinned context"></textarea>
                    </label>
                    <div class="memory-actions">
                        <button type="button" onclick={() => handleAddMemoryEntry('pinned_context')}>Pin</button>
                        <button type="button" onclick={() => handleAddMemoryEntry('decision')}>Decision</button>
                        <button type="button" onclick={() => handleAddMemoryEntry('next_step')}>Next</button>
                        <button type="button" onclick={() => handleAddMemoryEntry('session_summary')}>Session</button>
                    </div>

                    {#if handoffPrompt}
                        <label>
                            <span>Generated handoff prompt</span>
                            <textarea class="handoff-output" readonly rows="10" value={handoffPrompt}></textarea>
                        </label>
                    {/if}
                </div>

                <div class="panel-card notifications-panel">
                    <h2>Notifications</h2>
                    {#if workspaceNotifications(currentWorkspace()?.workspace_id ?? '').length > 0}
                        {#each workspaceNotifications(currentWorkspace()?.workspace_id ?? '') as notification}
                            <div class={`notification-item ${notification.read ? 'read' : 'unread'}`}>
                                <strong>{notification.level}</strong>
                                <span>{notification.message}</span>
                            </div>
                        {/each}
                    {:else}
                        <small class="hint">No notifications yet.</small>
                    {/if}
                </div>

                <div class="panel-card openflow-panel">
                    <div class="panel-card-header">
                        <h2>OpenFlow Runs</h2>
                    </div>
                    <label>
                        <span>Run title</span>
                        <input bind:value={openFlowTitleDraft} placeholder="Barbershop booking build" />
                    </label>
                    <label>
                        <span>Goal</span>
                        <textarea bind:value={openFlowGoalDraft} rows="3" placeholder="Describe what the orchestrator should build"></textarea>
                    </label>
                    <button type="button" onclick={handleCreateOpenFlowRun}>Create OpenFlow Run</button>

                    {#if $openflowRuntime?.active_runs?.length}
                        {#each $openflowRuntime.active_runs as run}
                            <div class="openflow-run-card">
                                <strong>{run.title}</strong>
                                <span>{run.status} - {run.current_phase}</span>
                                <small>{run.goal}</small>
                                <div class="openflow-run-actions">
                                    <button type="button" onclick={() => handleAdvanceOpenFlowRun(run.run_id)}>Advance</button>
                                    <button type="button" onclick={() => handleRetryOpenFlowRun(run.run_id)}>Retry</button>
                                </div>
                                <div class="openflow-run-meta">
                                    <span>Roles: {run.assigned_roles.join(', ')}</span>
                                    <span>Tasks: {run.task_graph.length}</span>
                                    <span>Resumable: {run.resumable ? 'yes' : 'no'}</span>
                                </div>
                            </div>
                        {/each}
                    {:else}
                        <small class="hint">No OpenFlow runs yet.</small>
                    {/if}
                </div>

                <div class="panel-card sessions-panel">
                    <div class="panel-card-header">
                        <h2>Sessions</h2>
                        <button type="button" onclick={handleCreateSession}>New</button>
                    </div>

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
                            <strong>{session.title}</strong>
                            <span>{session.shell ?? 'shell unknown'}</span>
                            <small>{session.state}</small>
                            <div class="session-actions">
                                <span>{session.cols}x{session.rows}</span>
                                <button type="button" onclick={(event) => {
                                    event.stopPropagation();
                                    void handleRestartSession(session.session_id);
                                }}>Restart</button>
                                <button type="button" onclick={(event) => {
                                    event.stopPropagation();
                                    void handleCloseSession(session.session_id);
                                }}>Close</button>
                            </div>
                        </div>
                    {/each}
                </div>
            </aside>

            <section class="workspace-main">
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
                        <p>Create a session or workspace to begin.</p>
                        <button type="button" onclick={handleCreateSession}>Create Session</button>
                    </div>
                {/if}
            </section>
        </section>
    {:else}
        <section class="loading-shell">
            <div class="loading-card">
                <h1>Loading Codemux state...</h1>
                <p>Waiting for backend state snapshot.</p>
            </div>
        </section>
    {/if}
</main>

<style>
    :global(html),
    :global(body) {
        width: 100%;
        height: 100%;
        margin: 0;
        padding: 0;
        background-color: var(--theme-background, #1a1b26);
        color: var(--theme-foreground, #a9b1d6);
        font-family: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, 'Open Sans', 'Helvetica Neue', sans-serif;
        overflow: hidden;
    }

    .app-container {
        display: flex;
        flex-direction: column;
        height: 100vh;
        width: 100vw;
        min-width: 0;
        min-height: 0;
    }

    .app-header {
        display: flex;
        align-items: flex-end;
        justify-content: space-between;
        padding: 18px 22px 14px;
        border-bottom: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        background:
            radial-gradient(circle at top left, color-mix(in srgb, var(--theme-accent, #7aa2f7) 16%, transparent), transparent 30%),
            var(--theme-background, #1a1b26);
    }

    .eyebrow {
        display: inline-block;
        margin-bottom: 6px;
        font-size: 0.72rem;
        letter-spacing: 0.16em;
        text-transform: uppercase;
        color: var(--theme-accent, #7aa2f7);
    }

    .app-header h1,
    .loading-card h1,
    .empty-card h2 {
        margin: 0;
    }

    .status-strip {
        display: flex;
        gap: 10px;
        font-size: 0.82rem;
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 78%, white 22%);
    }

    .status-strip span,
    .workspace-card,
    .session-card,
    .loading-card,
    .panel-card,
    .empty-card {
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 88%, white 12%);
        border-radius: 14px;
    }

    .status-strip span {
        padding: 8px 10px;
    }

    .workspace-shell {
        display: grid;
        grid-template-columns: 340px minmax(0, 1fr);
        flex: 1;
        min-height: 0;
    }

    .workspace-sidebar {
        display: flex;
        flex-direction: column;
        gap: 14px;
        padding: 18px;
        border-right: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 94%, black 6%);
        overflow: auto;
    }

    .panel-card {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 14px;
    }

    .panel-card h2 {
        margin: 0;
        font-size: 0.85rem;
        text-transform: uppercase;
        letter-spacing: 0.08em;
        color: var(--theme-accent, #7aa2f7);
    }

    .panel-card-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
    }

    .panel-card-header button,
    .details-panel button,
    .workspace-meta button,
    .session-actions button,
    .empty-card button {
        border: 1px solid color-mix(in srgb, var(--theme-accent, #7aa2f7) 28%, transparent);
        border-radius: 999px;
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 14%, transparent);
        color: var(--theme-foreground, #c0caf5);
        padding: 6px 10px;
        cursor: pointer;
    }

    .workspace-list,
    .sessions-panel {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .workspace-card,
    .session-card {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 12px;
        cursor: pointer;
    }

    .workspace-card.active,
    .session-card.active {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 50%, transparent);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--theme-accent, #7aa2f7) 28%, transparent);
    }

    .workspace-card.active:has(.notification-badge),
    .session-card.active {
        box-shadow:
            inset 0 0 0 1px color-mix(in srgb, var(--theme-accent, #7aa2f7) 28%, transparent),
            0 0 0 1px color-mix(in srgb, #f59e0b 35%, transparent);
    }

    .workspace-meta,
    .session-actions {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 8px;
        margin-top: 8px;
        font-size: 0.75rem;
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 72%, white 28%);
    }

    .details-panel label {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .toggle-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
    }

    .notification-badge {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        padding: 2px 8px;
        border-radius: 999px;
        background: color-mix(in srgb, #f59e0b 26%, transparent);
        color: #f8d08a;
    }

    .notification-item {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 10px 12px;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        border-radius: 12px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 92%, white 8%);
    }

    .notification-item.unread {
        border-color: color-mix(in srgb, #f59e0b 35%, transparent);
    }

    .notification-item.read {
        opacity: 0.72;
    }

    .details-panel input {
        width: 100%;
        box-sizing: border-box;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 14%, transparent);
        border-radius: 12px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 96%, white 4%);
        color: inherit;
        padding: 10px 12px;
    }

    .hint,
    .workspace-card span,
    .workspace-card small,
    .session-card span,
    .session-card small,
    .loading-card p,
    .empty-card p {
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 72%, white 28%);
    }

    .workspace-main {
        min-width: 0;
        min-height: 0;
        padding: 18px;
        overflow: auto;
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
        padding: 20px 22px;
    }

    .empty-card {
        min-height: 240px;
        flex-direction: column;
        gap: 12px;
    }

    @media (max-width: 900px) {
        .workspace-shell {
            grid-template-columns: 1fr;
        }

        .workspace-sidebar {
            max-height: 42vh;
            border-right: 0;
            border-bottom: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 10%, transparent);
        }
    }
</style>
