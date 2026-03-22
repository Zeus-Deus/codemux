<script lang="ts">
    import { onMount } from 'svelte';
    import { get } from 'svelte/store';
    import { theme, fallbackTheme, initTheme, shellAppearance, type ShellAppearance } from './stores/theme';
    import { paneDragState } from './stores/paneDrag';
    import { uiNotice, clearUiNotice, errorMessage, showUiNotice } from './stores/uiNotice';
    import { appState, initAppState } from './stores/core';
    import { initOpenFlowRuntime } from './stores/openflow';
    import { initProjectMemory } from './stores/memory';
    import { presetStore, initPresets } from './stores/presets';
    import {
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
        createTab,
        closeTab,
        activateTab,
    } from './stores/workspace';
    import type { SurfaceSnapshot, WorkspaceSnapshot, TabKind } from './stores/types';
    import PaneNode from './components/panes/PaneNode.svelte';
    import Sidebar from './components/sidebar/Sidebar.svelte';
    import NewWorkspaceLauncher from './components/sidebar/NewWorkspaceLauncher.svelte';
    import OpenFlowWorkspace from './components/openflow/OpenFlowWorkspace.svelte';
    import TabBar from './components/tabs/TabBar.svelte';
    import BrowserPane from './components/panes/BrowserPane.svelte';
    import DiffView from './components/diff/DiffView.svelte';
    import PresetBar from './components/presets/PresetBar.svelte';
    import PresetEditor from './components/presets/PresetEditor.svelte';
    import { findActiveSessionId } from './lib/paneTree';
    import type { TerminalPreset } from './stores/types';

    const themeKeys = [
        'accent', 'cursor', 'foreground', 'background',
        'selection_foreground', 'selection_background',
        'color0', 'color1', 'color2', 'color3', 'color4', 'color5',
        'color6', 'color7', 'color8', 'color9', 'color10', 'color11',
        'color12', 'color13', 'color14', 'color15'
    ] as const;

    let windowFocused = $state(true);
    let showNewWorkspaceLauncher = $state(false);
    let editingPreset = $state<TerminalPreset | null | undefined>(undefined);
    // undefined = editor closed, null = create mode, TerminalPreset = edit mode

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

    function isOpenFlowWorkspace(ws: any) {
        const type = ws?.workspace_type;
        return ws && type === 'open_flow';
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

    async function handleActivateTab(workspaceId: string, tabId: string) {
        try { await activateTab(workspaceId, tabId); } catch (e) { console.error('activate tab:', e); }
    }

    async function handleCloseTab(workspaceId: string, tabId: string) {
        try { await closeTab(workspaceId, tabId); } catch (e) { console.error('close tab:', e); }
    }

    async function handleCreateTab(workspaceId: string, kind: string) {
        try { await createTab(workspaceId, kind as TabKind); } catch (e) { console.error('create tab:', e); showUiNotice(errorMessage(e), 'error'); }
    }

    function activeWorkspaceForTabs(): WorkspaceSnapshot | null {
        if (!$appState) return null;
        const ws = $appState.workspaces.find((w) => w.workspace_id === $appState!.active_workspace_id);
        if (!ws || ws.workspace_type === 'open_flow') return null;
        return ws;
    }

    function surfaceForTab(workspace: WorkspaceSnapshot, surfaceId: string | null): SurfaceSnapshot | null {
        if (!surfaceId) return null;
        return workspace.surfaces.find((s) => s.surface_id === surfaceId) ?? null;
    }

    function handleWindowKeydown(event: KeyboardEvent) {
        if (!(event.metaKey || event.ctrlKey)) return;

        if (event.key === ']') { event.preventDefault(); void cycleWorkspace(1); return; }
        if (event.key === '[') { event.preventDefault(); void cycleWorkspace(-1); return; }

        // Tab shortcuts (Ctrl+T, Ctrl+W, Ctrl+1-9, Ctrl+Shift+B, Ctrl+Shift+D)
        const ws = activeWorkspaceForTabs();
        if (ws) {
            if (event.key.toLowerCase() === 't' && !event.shiftKey && !event.altKey) {
                event.preventDefault(); void handleCreateTab(ws.workspace_id, 'terminal'); return;
            }
            if (event.key.toLowerCase() === 'w' && !event.shiftKey && !event.altKey) {
                if (ws.tabs.length > 1) { event.preventDefault(); void handleCloseTab(ws.workspace_id, ws.active_tab_id); }
                return;
            }
            if (event.shiftKey && event.key.toLowerCase() === 'b' && !event.altKey) {
                event.preventDefault(); void handleCreateTab(ws.workspace_id, 'browser'); return;
            }
            if (event.shiftKey && event.key.toLowerCase() === 'd' && !event.altKey) {
                event.preventDefault(); void handleCreateTab(ws.workspace_id, 'diff'); return;
            }
            const numKey = parseInt(event.key);
            if (numKey >= 1 && numKey <= 9 && !event.shiftKey && !event.altKey) {
                const tabIndex = numKey - 1;
                if (tabIndex < ws.tabs.length) {
                    event.preventDefault(); void handleActivateTab(ws.workspace_id, ws.tabs[tabIndex].tab_id);
                }
                return;
            }
        }

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
        initPresets();

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
                    {#each $appState.workspaces as workspace (workspace.workspace_id)}
                        {@const isOf = isOpenFlowWorkspace(workspace)}
                        {@const activeTab = workspace.tabs.find(t => t.tab_id === workspace.active_tab_id)}
                        {#if isOf || workspace.tabs.length > 0 || workspace.surfaces.length > 0}
                            <div
                                class="workspace-surface-layer"
                                class:active={workspace.workspace_id === $appState.active_workspace_id}
                                aria-hidden={workspace.workspace_id === $appState.active_workspace_id ? 'false' : 'true'}
                            >
                                {#if isOf}
                                    <OpenFlowWorkspace {workspace} />
                                {:else}
                                    <div class="workspace-tabbed-layout">
                                        {#if workspace.tabs.length > 0}
                                            <TabBar
                                                tabs={workspace.tabs}
                                                activeTabId={workspace.active_tab_id}
                                                workspaceId={workspace.workspace_id}
                                                on:activate={(e) => handleActivateTab(workspace.workspace_id, e.detail.tabId)}
                                                on:close={(e) => handleCloseTab(workspace.workspace_id, e.detail.tabId)}
                                                on:create={(e) => handleCreateTab(workspace.workspace_id, e.detail.kind)}
                                            />
                                        {/if}
                                        {#if $presetStore && (!activeTab || activeTab.kind === 'terminal')}
                                            <PresetBar
                                                workspaceId={workspace.workspace_id}
                                                presets={$presetStore.presets}
                                                onEditPreset={(p) => { editingPreset = p; }}
                                            />
                                        {/if}
                                        <div class="tab-content">
                                            {#if !activeTab || activeTab.kind === 'terminal'}
                                                {@const surface = (activeTab?.surface_id
                                                    ? surfaceForTab(workspace, activeTab.surface_id)
                                                    : null) ?? surfaceForWorkspace(workspace)}
                                                {#if surface && surface.root}
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
                                                {/if}
                                            {:else if activeTab.kind === 'browser' && activeTab.browser_id}
                                                <BrowserPane browserId={activeTab.browser_id} />
                                            {:else if activeTab.kind === 'diff'}
                                                <DiffView workspaceCwd={workspace.cwd} />
                                            {/if}
                                        </div>
                                    </div>
                                {/if}
                            </div>
                        {/if}
                    {/each}
                    {#if !$appState.workspaces.some(w => isOpenFlowWorkspace(w) || surfaceForWorkspace(w))}
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

{#if editingPreset !== undefined}
    <PresetEditor preset={editingPreset} on:close={() => { editingPreset = undefined; }} />
{/if}

<style>
    :global(html),
    :global(body) {
        /* ---- Typography ---- */
        --ui-font-sans: -apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', system-ui, sans-serif;
        --ui-font-mono: 'JetBrainsMono Nerd Font', 'JetBrains Mono', 'Fira Code', 'Cascadia Code', ui-monospace, monospace;

        /* ---- Shell palette (fixed neutral dark — never changes with theme) ---- */
        --ui-layer-0: #0d0f11;
        --ui-layer-1: #151719;
        --ui-layer-2: #1c1e22;
        --ui-layer-3: #252830;
        --ui-border-soft: rgba(255, 255, 255, 0.08);
        --ui-border-strong: rgba(255, 255, 255, 0.14);
        --ui-text-primary: #e0e0e0;
        --ui-text-secondary: #9a9a9a;
        --ui-text-muted: #636363;

        /* ---- Accent tokens (theme-reactive — only theme color in the shell) ---- */
        --ui-accent: var(--theme-accent, #7aa2f7);
        --ui-accent-soft: color-mix(in srgb, var(--theme-accent, #7aa2f7) 18%, transparent 82%);
        --ui-success: var(--theme-color2, #9ece6a);
        --ui-danger: var(--theme-color1, #f7768e);
        --ui-attention: var(--theme-color3, #e0af68);
        --ui-attention-soft: color-mix(in srgb, var(--theme-color3, #e0af68) 14%, transparent 86%);

        /* ---- Legacy aliases used by pane components ---- */
        --ui-app-bg: var(--ui-layer-0);
        --ui-sidebar-bg: var(--ui-layer-1);
        --ui-surface: var(--ui-layer-2);
        --ui-surface-strong: var(--ui-layer-3);
        --ui-pane-bg: var(--ui-layer-0);
        --ui-pane-bg-strong: var(--ui-layer-1);

        /* ---- Geometry ---- */
        --ui-radius-sm: 4px;
        --ui-radius-md: 6px;
        --ui-radius-lg: 8px;
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
        font-family: var(--ui-font-sans);
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

    .workspace-tabbed-layout {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
    }

    .tab-content {
        flex: 1;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
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
        border-radius: var(--ui-radius-lg);
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
        border-radius: var(--ui-radius-lg);
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
        border-radius: var(--ui-radius-md);
        cursor: pointer;
        transition: opacity 0.15s;
    }

    .empty-create-btn:hover {
        opacity: 0.9;
    }
</style>
