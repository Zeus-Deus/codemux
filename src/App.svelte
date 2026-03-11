<script lang="ts">
    import { onMount } from 'svelte';
    import { get } from 'svelte/store';
    import { theme, fallbackTheme, initTheme, shellAppearance, type ShellAppearance } from './stores/theme';
    import { paneDragState } from './stores/paneDrag';
    import { uiNotice, clearUiNotice, errorMessage, showUiNotice } from './stores/uiNotice';
    import {
        appState,
        initAppState,
        openflowRuntime,
        initOpenFlowRuntime,
        projectMemory,
        initProjectMemory,
        activatePane,
        closePane,
        cyclePane,
        cycleWorkspace,
        resizeActivePane,
        resizeSplit,
        markWorkspaceNotificationsRead,
        splitPane,
        createBrowserPane,
        swapPanes,
        type SurfaceSnapshot,
        type WorkspaceSnapshot
    } from './stores/appState';
    import PaneNode from './components/panes/PaneNode.svelte';
    import Sidebar from './components/sidebar/Sidebar.svelte';
    import NewWorkspaceLauncher from './components/sidebar/NewWorkspaceLauncher.svelte';
    import { findActiveSessionId } from './lib/paneTree';

    const themeKeys = [
        'accent', 'cursor', 'foreground', 'background',
        'selection_foreground', 'selection_background',
        'color0', 'color1', 'color2', 'color3', 'color4', 'color5',
        'color6', 'color7', 'color8', 'color9', 'color10', 'color11',
        'color12', 'color13', 'color14', 'color15'
    ] as const;

    let windowFocused = $state(true);
    let showNewWorkspaceLauncher = $state(false);

    function applyThemeVars(nextTheme = fallbackTheme) {
        const root = document.documentElement;
        for (const key of themeKeys) {
            root.style.setProperty(`--theme-${key.replaceAll('_', '-')}`, nextTheme[key]);
        }
    }

    function applyShellAppearance(fontFamily: string | null | undefined) {
        const root = document.documentElement;
        root.style.setProperty('--shell-font-family', fontFamily?.trim() || 'monospace');
    }

    function currentWorkspace() {
        return $appState?.workspaces.find((w) => w.workspace_id === $appState?.active_workspace_id) ?? null;
    }

    function currentSurface() {
        const ws = currentWorkspace();
        return ws?.surfaces.find((s) => s.surface_id === ws.active_surface_id) ?? null;
    }

    function surfaceForWorkspace(workspace: WorkspaceSnapshot | null): SurfaceSnapshot | null {
        return workspace?.surfaces.find((surface) => surface.surface_id === workspace.active_surface_id) ?? null;
    }

    async function handleActivatePane(paneId: string) {
        try { await activatePane(paneId); } catch (e) { console.error('activate pane:', e); }
    }

    async function handleSplitPane(paneId: string, direction: 'horizontal' | 'vertical') {
        try { await splitPane(paneId, direction); } catch (e) { console.error('split pane:', e); showUiNotice(errorMessage(e), 'error'); }
    }

    async function handleClosePane(paneId: string) {
        try { await closePane(paneId); } catch (e) { console.error('close pane:', e); }
    }

    async function handleResizeSplit(paneId: string, childSizes: number[]) {
        try { await resizeSplit(paneId, childSizes); } catch (e) { console.error('resize split:', e); }
    }

    async function handleCreateBrowserPane(paneId: string) {
        try { await createBrowserPane(paneId); } catch (e) { console.error('create browser pane:', e); showUiNotice(errorMessage(e), 'error'); }
    }

    async function handleSwapPanes(sourcePaneId: string, targetPaneId: string) {
        try { await swapPanes(sourcePaneId, targetPaneId); } catch (e) { console.error('swap panes:', e); }
    }

    function handleWindowKeydown(event: KeyboardEvent) {
        if (!(event.metaKey || event.ctrlKey)) return;

        if (event.key === ']') { event.preventDefault(); void cycleWorkspace(1); return; }
        if (event.key === '[') { event.preventDefault(); void cycleWorkspace(-1); return; }

        if (event.shiftKey && event.key.toLowerCase() === 'j') { event.preventDefault(); void cyclePane(1); return; }
        if (event.shiftKey && event.key.toLowerCase() === 'k') { event.preventDefault(); void cyclePane(-1); return; }
        if (event.key.toLowerCase() === 'l') { event.preventDefault(); void cyclePane(1); return; }
        if (event.key.toLowerCase() === 'h') { event.preventDefault(); void cyclePane(-1); return; }

        if (event.altKey && (event.key === 'ArrowLeft' || event.key.toLowerCase() === 'h')) { event.preventDefault(); void resizeActivePane(-0.05); return; }
        if (event.altKey && (event.key === 'ArrowRight' || event.key.toLowerCase() === 'l')) { event.preventDefault(); void resizeActivePane(0.05); return; }
        if (event.altKey && (event.key === 'ArrowUp' || event.key.toLowerCase() === 'k')) { event.preventDefault(); void resizeActivePane(-0.05); return; }
        if (event.altKey && (event.key === 'ArrowDown' || event.key.toLowerCase() === 'j')) { event.preventDefault(); void resizeActivePane(0.05); }
    }

    onMount(() => {
        applyThemeVars(get(theme) ?? fallbackTheme);
        initTheme();
        initAppState();
        initProjectMemory();
        initOpenFlowRuntime();

        const themeUnsub = theme.subscribe((t) => applyThemeVars(t ?? fallbackTheme));
        const shellAppearanceUnsub = shellAppearance.subscribe((appearance: ShellAppearance | null) => {
            applyShellAppearance(appearance?.font_family);
        });

        window.addEventListener('keydown', handleWindowKeydown);

        const onFocus = () => {
            windowFocused = true;
            const ws = currentWorkspace();
            if (ws) void markWorkspaceNotificationsRead(ws.workspace_id).catch(() => {});
        };
        const onBlur = () => { windowFocused = false; };
        window.addEventListener('focus', onFocus);
        window.addEventListener('blur', onBlur);

        return () => {
            themeUnsub();
            shellAppearanceUnsub();
            window.removeEventListener('keydown', handleWindowKeydown);
            window.removeEventListener('focus', onFocus);
            window.removeEventListener('blur', onBlur);
        };
    });
</script>

<main class="app-shell">
    {#if $appState && $appState.workspaces.length > 0}
        <div class="workspace-shell">
            <Sidebar {windowFocused} />

            <section class="workspace-main">
                <div class="workspace-stage">
                    {#if currentSurface()}
                        {#each $appState.workspaces as workspace (workspace.workspace_id)}
                            {@const surface = surfaceForWorkspace(workspace)}
                            {#if surface}
                                <div
                                    class="workspace-surface-layer"
                                    class:active={workspace.workspace_id === $appState.active_workspace_id}
                                    aria-hidden={workspace.workspace_id === $appState.active_workspace_id ? 'false' : 'true'}
                                >
                                    <PaneNode
                                        node={surface.root}
                                        activePaneId={surface.active_pane_id}
                                        on:activate={(e) => handleActivatePane(e.detail.paneId)}
                                        on:split={(e) => handleSplitPane(e.detail.paneId, e.detail.direction)}
                                        on:close={(e) => handleClosePane(e.detail.paneId)}
                                        on:resize={(e) => handleResizeSplit(e.detail.paneId, e.detail.childSizes)}
                                        on:browser={(e) => handleCreateBrowserPane(e.detail.paneId)}
                                        on:swap={(e) => handleSwapPanes(e.detail.sourcePaneId, e.detail.targetPaneId)}
                                    />
                                </div>
                            {/if}
                        {/each}
                    {:else}
                        <div class="empty-stage">
                            <div class="empty-stage-card">
                                <div class="empty-stage-icon">
                                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                                        <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
                                        <path d="M6 9l3 3-3 3M11 15h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                                    </svg>
                                </div>
                                <h2>No active surface</h2>
                                <p>Create a shell session to start working.</p>
                            </div>
                        </div>
                    {/if}
                </div>
            </section>
        </div>

        {#if $uiNotice}
            <div class="global-notice-wrap" aria-live="polite">
                <div class="global-notice" class:error={$uiNotice.kind === 'error'}>
                    <span>{$uiNotice.message}</span>
                    <button type="button" class="global-notice-close" onclick={clearUiNotice} aria-label="Dismiss message">
                        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                        </svg>
                    </button>
                </div>
            </div>
        {/if}
    {:else}
        <div class="empty-shell">
            <div class="empty-card">
                <div class="empty-icon">
                    <svg width="48" height="48" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                        <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
                        <path d="M6 9l3 3-3 3M11 15h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                </div>
                <h2>No workspace</h2>
                <p>Create a workspace to start working.</p>
                <button class="empty-create-btn" onclick={() => showNewWorkspaceLauncher = true}>
                    Create workspace
                </button>
            </div>
        </div>
    {/if}
</main>

{#if showNewWorkspaceLauncher}
    <NewWorkspaceLauncher on:close={() => showNewWorkspaceLauncher = false} />
{/if}

<style>
    :global(html),
    :global(body) {
        /* ---- Accent & attention ---- */
        --ui-accent: var(--theme-accent, #7aa2f7);
        --ui-accent-soft: rgba(122, 162, 247, 0.18);
        --ui-attention: var(--theme-color11, #e0af68);
        --ui-attention-soft: rgba(224, 175, 104, 0.14);
        --ui-success: var(--theme-color10, #9ece6a);
        --ui-danger: var(--theme-color1, #f7768e);

        /* ---- Layer system ---- */
        --ui-layer-0: var(--theme-background, #1a1b26);
        --ui-layer-1: #161925;
        --ui-layer-2: #1d2231;
        --ui-layer-3: #252c3f;

        /* Legacy aliases used by pane components */
        --ui-app-bg: var(--ui-layer-0);
        --ui-sidebar-bg: var(--ui-layer-1);
        --ui-surface: var(--ui-layer-2);
        --ui-surface-strong: var(--ui-layer-3);
        --ui-pane-bg: var(--ui-layer-0);
        --ui-pane-bg-strong: var(--ui-layer-1);

        /* ---- Borders ---- */
        --ui-border-soft: rgba(192, 202, 245, 0.1);
        --ui-border-strong: rgba(192, 202, 245, 0.18);

        /* ---- Text hierarchy ---- */
        --ui-text-primary: var(--theme-foreground, #c0caf5);
        --ui-text-secondary: #9aa4c2;
        --ui-text-muted: #6f7893;

        /* ---- Geometry ---- */
        --ui-radius-sm: 6px;
        --ui-radius-md: 8px;
        --ui-radius-lg: 10px;
        --ui-sidebar-width: 240px;

        /* ---- Motion ---- */
        --ui-motion-fast: 120ms ease-out;
        --ui-motion-base: 160ms ease-out;
        --ui-motion-slow: 240ms ease-out;

        width: 100%;
        height: 100%;
        margin: 0;
        padding: 0;
        background-color: var(--ui-layer-0);
        background: var(--ui-layer-0);
        color: var(--ui-text-primary);
        font-family: var(--shell-font-family, 'JetBrainsMono Nerd Font'), 'JetBrains Mono', 'SFMono-Regular', Menlo, Consolas, monospace;
        font-size: 13px;
        overflow: hidden;
    }

    :global(body) {
        background: var(--ui-layer-0);
        background-color: var(--ui-layer-0);
    }

    :global(body > div) {
        width: 100%;
        height: 100%;
        background: var(--ui-layer-0);
        background-color: var(--ui-layer-0);
    }

    .app-shell {
        display: flex;
        flex-direction: column;
        width: 100vw;
        height: 100dvh;
        min-width: 0;
        min-height: 0;
    }

    .workspace-shell {
        display: flex;
        flex-direction: row;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .workspace-main {
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        background: var(--ui-layer-0);
    }

    .workspace-stage {
        position: relative;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .workspace-surface-layer {
        position: absolute;
        inset: 0;
        visibility: hidden;
        opacity: 0;
        pointer-events: none;
    }

    .workspace-surface-layer.active {
        visibility: visible;
        opacity: 1;
        pointer-events: auto;
        z-index: 1;
    }

    .global-notice-wrap {
        position: absolute;
        right: 18px;
        bottom: 18px;
        z-index: 30;
        pointer-events: none;
    }

    .global-notice {
        display: flex;
        align-items: center;
        gap: 10px;
        max-width: min(520px, calc(100vw - 36px));
        padding: 10px 12px;
        border-radius: 10px;
        border: 1px solid color-mix(in srgb, var(--ui-accent) 26%, transparent);
        background: color-mix(in srgb, var(--ui-layer-2) 92%, black 8%);
        box-shadow: 0 12px 30px rgba(0, 0, 0, 0.28);
        color: var(--ui-text-primary);
        pointer-events: auto;
    }

    .global-notice.error {
        border-color: color-mix(in srgb, var(--ui-danger) 42%, transparent);
        background: color-mix(in srgb, var(--ui-danger) 10%, var(--ui-layer-2) 90%);
    }

    .global-notice span {
        font-size: 0.8rem;
        line-height: 1.45;
    }

    .global-notice-close {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 22px;
        height: 22px;
        padding: 0;
        border: 0;
        border-radius: 999px;
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        flex-shrink: 0;
    }

    .global-notice-close:hover {
        background: color-mix(in srgb, var(--ui-layer-3) 70%, transparent);
        color: var(--ui-text-primary);
    }

    /* ---- Empty state ---- */

    .empty-stage {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 100%;
        height: 100%;
        padding: 32px;
        box-sizing: border-box;
    }

    .empty-stage-card {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 10px;
        text-align: center;
        max-width: 320px;
    }

    .empty-stage-icon {
        width: 48px;
        height: 48px;
        border-radius: 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--ui-text-muted);
    }

    .empty-stage-card h2 {
        margin: 0;
        font-size: 1rem;
        font-weight: 600;
        color: var(--ui-text-secondary);
    }

    .empty-stage-card p {
        margin: 0;
        font-size: 0.82rem;
        color: var(--ui-text-muted);
        line-height: 1.5;
    }

    /* ---- Empty state ---- */

    .empty-shell {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: 1;
        width: 100%;
        height: 100%;
        background: var(--ui-layer-0);
    }

    .empty-card {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 12px;
        text-align: center;
    }

    .empty-icon {
        color: var(--ui-text-muted);
    }

    .empty-card h2 {
        margin: 0;
        font-size: 1.1rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .empty-card p {
        margin: 0;
        font-size: 0.85rem;
        color: var(--ui-text-muted);
    }

    .empty-create-btn {
        margin-top: 8px;
        padding: 8px 16px;
        font-size: 0.85rem;
        font-weight: 500;
        color: var(--ui-layer-0);
        background: var(--ui-accent);
        border: none;
        border-radius: 6px;
        cursor: pointer;
        transition: opacity 0.15s;
    }

    .empty-create-btn:hover {
        opacity: 0.9;
    }
</style>
