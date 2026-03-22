<script lang="ts">
    import { appState } from '../../stores/core';
    import { onMount } from 'svelte';
    import {
        activateWorkspace,
        closeWorkspace,
        closeWorkspaceWithWorktree,
        markWorkspaceNotificationsRead,
        renameWorkspace,
        createTerminalSession,
        notifyAttention,
        setNotificationSoundEnabled,
        detectEditors,
        openInEditor,
        getWorkspaceConfig,
        createSection,
        renameSection,
        deleteSection,
        setSectionColor,
        toggleSectionCollapsed,
        moveWorkspaceToSection,
        reorderWorkspaces,
        reorderSections,
    } from '../../stores/workspace';
    import type { EditorInfo, LayoutPreset, WorkspaceTemplateKind } from '../../stores/types';
    import { SECTION_PRESET_COLORS } from '../../stores/types';
    import { findActiveSessionId } from '../../lib/paneTree';
    import WorkspaceRow from './WorkspaceRow.svelte';
    import SectionHeader from './SectionHeader.svelte';
    import NotificationsSection from './NotificationsSection.svelte';
    import PortsSection from './PortsSection.svelte';
    import OpenFlowLauncher from './OpenFlowLauncher.svelte';
    import MemoryDrawer from './MemoryDrawer.svelte';
    import NewWorkspaceLauncher from './NewWorkspaceLauncher.svelte';
    import { errorMessage, showUiNotice } from '../../stores/uiNotice';

    let { windowFocused }: { windowFocused: boolean } = $props();

    let editors = $state<EditorInfo[]>([]);

    onMount(() => {
        detectEditors().then((result) => { editors = result; }).catch(() => {});
    });

    function handleOpenInEditor(cwd: string) {
        if (editors.length === 1) {
            void openInEditor(editors[0].id, cwd);
        } else if (editors.length > 1) {
            // Default to first editor; multi-editor dropdown can be added later
            void openInEditor(editors[0].id, cwd);
        }
    }

    let confirmingDelete = $state<{ workspaceId: string; worktreePath: string; hasTeardown: boolean } | null>(null);
    let teardownError = $state<{ workspaceId: string; message: string; removeWorktree: boolean } | null>(null);

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
        const ws = $appState?.workspaces.find(w => w.workspace_id === workspaceId);
        if (ws?.worktree_path) {
            const config = await getWorkspaceConfig(ws.worktree_path).catch(() => null);
            confirmingDelete = {
                workspaceId,
                worktreePath: ws.worktree_path,
                hasTeardown: (config?.teardown?.length ?? 0) > 0,
            };
            return;
        }
        try {
            await closeWorkspace(workspaceId);
        } catch (error) {
            const msg = errorMessage(error);
            if (msg.includes('Teardown failed')) {
                teardownError = { workspaceId, message: msg, removeWorktree: false };
            } else {
                console.error('Failed to close workspace:', error);
                showUiNotice(msg, 'error');
            }
        }
    }

    async function handleConfirmDelete(removeWorktree: boolean) {
        if (!confirmingDelete) return;
        const { workspaceId } = confirmingDelete;
        confirmingDelete = null;
        try {
            await closeWorkspaceWithWorktree(workspaceId, removeWorktree);
        } catch (error) {
            const msg = errorMessage(error);
            if (msg.includes('Teardown failed')) {
                teardownError = { workspaceId, message: msg, removeWorktree };
            } else {
                console.error('Failed to close workspace:', error);
                showUiNotice(msg, 'error');
            }
        }
    }

    async function handleForceDelete() {
        if (!teardownError) return;
        const { workspaceId, removeWorktree } = teardownError;
        teardownError = null;
        try {
            const ws = $appState?.workspaces.find(w => w.workspace_id === workspaceId);
            if (ws?.worktree_path) {
                await closeWorkspaceWithWorktree(workspaceId, removeWorktree, true);
            } else {
                await closeWorkspace(workspaceId, true);
            }
        } catch (error) {
            console.error('Force delete failed:', error);
            showUiNotice(errorMessage(error), 'error');
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
            showUiNotice(errorMessage(error), 'error');
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

    // ---- Workspace sections ----

    const sectionedWorkspaces = $derived.by(() => {
        if (!$appState) return { sectionGroups: [], unsorted: [] };

        const sections = [...$appState.sections].sort((a, b) => a.position - b.position);
        const assignedIds = new Set(sections.flatMap(s => s.workspace_ids));
        const unsorted = $appState.workspaces.filter(w => !assignedIds.has(w.workspace_id));

        const sectionGroups = sections.map(section => ({
            section,
            workspaces: section.workspace_ids
                .map(id => $appState!.workspaces.find(w => w.workspace_id === id))
                .filter((w): w is NonNullable<typeof w> => w != null),
        }));

        return { sectionGroups, unsorted };
    });

    async function handleCreateSection() {
        const color = SECTION_PRESET_COLORS[
            ($appState?.sections.length ?? 0) % SECTION_PRESET_COLORS.length
        ];
        try {
            await createSection('New Section', color);
        } catch (error) {
            console.error('Failed to create section:', error);
        }
    }

    async function handleToggleSectionCollapse(sectionId: string) {
        try { await toggleSectionCollapsed(sectionId); } catch (e) { console.error(e); }
    }

    async function handleRenameSection(sectionId: string, name: string) {
        try { await renameSection(sectionId, name); } catch (e) { console.error(e); }
    }

    async function handleSetSectionColor(sectionId: string, color: string) {
        try { await setSectionColor(sectionId, color); } catch (e) { console.error(e); }
    }

    async function handleDeleteSection(sectionId: string) {
        try { await deleteSection(sectionId); } catch (e) { console.error(e); }
    }

    async function handleMoveWorkspaceToSection(workspaceId: string, sectionId: string | null) {
        try { await moveWorkspaceToSection(workspaceId, sectionId); } catch (e) { console.error(e); }
    }

    // ---- Drag and drop ----

    let dragState = $state<{
        type: 'workspace' | 'section';
        id: string;
        sourceSectionId: string | null;
    } | null>(null);

    let dropIndicatorY = $state<number | null>(null);
    let dropTarget = $state<{
        sectionId: string | null;
        index: number;
    } | null>(null);

    let workspaceListEl = $state<HTMLDivElement | null>(null);

    function handleWorkspaceDragStart(workspaceId: string, sectionId: string | null) {
        return (e: DragEvent) => {
            dragState = { type: 'workspace', id: workspaceId, sourceSectionId: sectionId };
            e.dataTransfer!.effectAllowed = 'move';
            e.dataTransfer!.setData('text/plain', workspaceId);
        };
    }

    function handleSectionDragStart(sectionId: string) {
        return (e: DragEvent) => {
            dragState = { type: 'section', id: sectionId, sourceSectionId: null };
            e.dataTransfer!.effectAllowed = 'move';
            e.dataTransfer!.setData('text/plain', sectionId);
        };
    }

    function handleDragOver(e: DragEvent) {
        if (!dragState || !workspaceListEl) return;
        e.preventDefault();
        e.dataTransfer!.dropEffect = 'move';

        if (dragState.type === 'workspace') {
            computeWorkspaceDropTarget(e.clientY);
        } else {
            computeSectionDropTarget(e.clientY);
        }
    }

    function computeWorkspaceDropTarget(clientY: number) {
        if (!workspaceListEl) return;
        const listRect = workspaceListEl.getBoundingClientRect();

        // Step 1: Find which zone the cursor is in
        let targetZone: { el: HTMLElement; sectionId: string | null } | null = null;

        const unsortedZone = workspaceListEl.querySelector<HTMLElement>('[data-drop-zone="unsorted"]');
        if (unsortedZone) {
            const rect = unsortedZone.getBoundingClientRect();
            if (clientY >= rect.top && clientY <= rect.bottom) {
                targetZone = { el: unsortedZone, sectionId: null };
            }
        }

        if (!targetZone) {
            const sectionZones = workspaceListEl.querySelectorAll<HTMLElement>('[data-drop-zone-section]');
            for (const zone of sectionZones) {
                const rect = zone.getBoundingClientRect();
                if (clientY >= rect.top && clientY <= rect.bottom) {
                    targetZone = { el: zone, sectionId: zone.dataset.dropZoneSection || null };
                    break;
                }
            }
        }

        // Fallback: find the closest zone if cursor is between zones
        if (!targetZone) {
            const allZones: Array<{ el: HTMLElement; sectionId: string | null }> = [];
            if (unsortedZone) allZones.push({ el: unsortedZone, sectionId: null });
            const sectionZones = workspaceListEl.querySelectorAll<HTMLElement>('[data-drop-zone-section]');
            for (const zone of sectionZones) {
                allZones.push({ el: zone, sectionId: zone.dataset.dropZoneSection || null });
            }
            let closestDist = Infinity;
            for (const zone of allZones) {
                const rect = zone.el.getBoundingClientRect();
                const dist = Math.min(Math.abs(clientY - rect.top), Math.abs(clientY - rect.bottom));
                if (dist < closestDist) {
                    closestDist = dist;
                    targetZone = zone;
                }
            }
        }

        if (!targetZone) return;

        // Step 2: Find insertion position within the target zone
        const rows = targetZone.el.querySelectorAll<HTMLElement>('[data-ws-id]');

        if (rows.length === 0) {
            // Empty/collapsed section — drop at index 0
            dropTarget = { sectionId: targetZone.sectionId, index: 0 };
            const zoneRect = targetZone.el.getBoundingClientRect();
            dropIndicatorY = zoneRect.bottom - listRect.top;
            return;
        }

        let closestEl: HTMLElement | null = null;
        let closestDist = Infinity;
        let insertBefore = true;

        for (const row of rows) {
            const rect = row.getBoundingClientRect();
            const midY = rect.top + rect.height / 2;
            const dist = Math.abs(clientY - midY);
            if (dist < closestDist) {
                closestDist = dist;
                closestEl = row;
                insertBefore = clientY < midY;
            }
        }

        if (!closestEl) return;

        const indexInZone = parseInt(closestEl.dataset.wsIndex ?? '0', 10);
        const targetIndex = insertBefore ? indexInZone : indexInZone + 1;

        dropTarget = { sectionId: targetZone.sectionId, index: targetIndex };

        const elRect = closestEl.getBoundingClientRect();
        dropIndicatorY = (insertBefore ? elRect.top : elRect.bottom) - listRect.top;
    }

    function computeSectionDropTarget(clientY: number) {
        if (!workspaceListEl) return;
        const headers = workspaceListEl.querySelectorAll<HTMLElement>('[data-section-header-id]');
        if (headers.length === 0) return;

        let closestEl: HTMLElement | null = null;
        let closestDist = Infinity;
        let insertBefore = true;

        for (const header of headers) {
            const rect = header.getBoundingClientRect();
            const midY = rect.top + rect.height / 2;
            const dist = Math.abs(clientY - midY);
            if (dist < closestDist) {
                closestDist = dist;
                closestEl = header;
                insertBefore = clientY < midY;
            }
        }

        if (!closestEl) return;
        const headerIdx = parseInt(closestEl.dataset.sectionIdx ?? '0', 10);
        const targetIndex = insertBefore ? headerIdx : headerIdx + 1;
        dropTarget = { sectionId: null, index: targetIndex };

        const listRect = workspaceListEl.getBoundingClientRect();
        const elRect = closestEl.getBoundingClientRect();
        dropIndicatorY = (insertBefore ? elRect.top : elRect.bottom) - listRect.top;
    }

    async function handleDrop(e: DragEvent) {
        e.preventDefault();
        if (!dragState || !dropTarget) {
            clearDrag();
            return;
        }

        try {
            if (dragState.type === 'workspace') {
                if (dropTarget.sectionId !== null) {
                    // Dropping into a named section — use move with position
                    await moveWorkspaceToSection(dragState.id, dropTarget.sectionId, dropTarget.index);
                } else {
                    // Dropping into unsorted zone
                    // First remove from any section if needed
                    if (dragState.sourceSectionId !== null) {
                        await moveWorkspaceToSection(dragState.id, null);
                    }
                    // Reorder the main workspaces array to place at desired position among unsorted
                    if ($appState) {
                        const sections = $appState.sections;
                        const assignedIds = new Set(sections.flatMap(s => s.workspace_ids));
                        // After moving, the dragged workspace is unsorted — remove it from assigned
                        assignedIds.delete(dragState.id);
                        const unsortedIds = $appState.workspaces
                            .map(w => w.workspace_id)
                            .filter(id => !assignedIds.has(id) && id !== dragState!.id);
                        // Insert at desired position among unsorted
                        const idx = Math.min(dropTarget.index, unsortedIds.length);
                        unsortedIds.splice(idx, 0, dragState.id);
                        // Build full order: unsorted in new order + sectioned in original order
                        const sectionedIds = $appState.workspaces
                            .map(w => w.workspace_id)
                            .filter(id => assignedIds.has(id));
                        await reorderWorkspaces([...unsortedIds, ...sectionedIds]);
                    }
                }
            } else if (dragState.type === 'section') {
                const currentOrder = sectionedWorkspaces.sectionGroups.map(g => g.section.section_id);
                const dragIdx = currentOrder.indexOf(dragState.id);
                if (dragIdx >= 0) {
                    const newOrder = [...currentOrder];
                    newOrder.splice(dragIdx, 1);
                    const insertIdx = Math.min(dropTarget.index, newOrder.length);
                    newOrder.splice(insertIdx > dragIdx ? insertIdx - 1 : insertIdx, 0, dragState.id);
                    await reorderSections(newOrder);
                }
            }
        } catch (e) {
            console.error('Drop failed:', e);
        }

        clearDrag();
    }

    function handleDragEnd() {
        clearDrag();
    }

    function clearDrag() {
        dragState = null;
        dropTarget = null;
        dropIndicatorY = null;
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

    {#if confirmingDelete}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="confirm-backdrop" onclick={() => { confirmingDelete = null; }} onkeydown={() => {}}>
            <div class="confirm-dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
                <p class="confirm-title">Delete worktree?</p>
                <p class="confirm-text">This workspace uses a git worktree. Delete the worktree and branch too?</p>
                {#if confirmingDelete.hasTeardown}
                    <p class="confirm-hint">Teardown scripts will run before deletion.</p>
                {/if}
                <div class="confirm-actions">
                    <button class="confirm-btn" onclick={() => { confirmingDelete = null; }}>Cancel</button>
                    <button class="confirm-btn" onclick={() => handleConfirmDelete(false)}>Keep worktree</button>
                    <button class="confirm-btn confirm-danger" onclick={() => handleConfirmDelete(true)}>Delete worktree</button>
                </div>
            </div>
        </div>
    {/if}

    {#if teardownError}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="confirm-backdrop" onclick={() => { teardownError = null; }} onkeydown={() => {}}>
            <div class="confirm-dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
                <p class="confirm-title">Teardown failed</p>
                <p class="confirm-text teardown-error-text">{teardownError.message}</p>
                <div class="confirm-actions">
                    <button class="confirm-btn" onclick={() => { teardownError = null; }}>Cancel</button>
                    <button class="confirm-btn confirm-danger" onclick={handleForceDelete}>Force delete</button>
                </div>
            </div>
        </div>
    {/if}

    <!-- Brand + active workspace header -->
    <header class="sidebar-head">
        <div class="brand-row">
            <div class="brand-mark">
                <span class="brand-diamond"></span>
                <span class="brand-name">Codemux</span>
            </div>
            <div class="brand-actions">
                <button
                    class="icon-btn"
                    type="button"
                    title="New section"
                    onclick={handleCreateSection}
                    aria-label="New section"
                >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                        <path d="M1 3h10M1 6h7M1 9h10" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
                    </svg>
                </button>
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
                {#if activeWorkspace.git_branch}
                    <span class="active-git-info">
                        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                            <path d="M6 3v12M18 9v12M6 3C6 3 6 9 12 9s6-6 6-6M6 15c0 0 0 6 6 6s6-6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                        </svg>
                        {activeWorkspace.git_branch}
                        {#if activeWorkspace.git_additions > 0}
                            <span class="head-diff-add">+{activeWorkspace.git_additions}</span>
                        {/if}
                        {#if activeWorkspace.git_deletions > 0}
                            <span class="head-diff-del">-{activeWorkspace.git_deletions}</span>
                        {/if}
                    </span>
                {/if}
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
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="workspace-list"
        bind:this={workspaceListEl}
        ondragover={handleDragOver}
        ondrop={handleDrop}
        ondragend={handleDragEnd}
    >
        {#if $appState?.workspaces.length}
            <!-- Unsorted workspaces (not in any section) -->
            <div data-drop-zone="unsorted">
                {#if sectionedWorkspaces.unsorted.length > 0}
                    {#if sectionedWorkspaces.sectionGroups.length > 0}
                        <div class="unsorted-label">Unsorted</div>
                    {/if}
                    {#each sectionedWorkspaces.unsorted as workspace, idx (workspace.workspace_id)}
                        <div data-ws-id={workspace.workspace_id} data-ws-index={idx}>
                            <WorkspaceRow
                                {workspace}
                                isActive={workspace.workspace_id === $appState.active_workspace_id}
                                sections={$appState.sections}
                                currentSectionId={null}
                                isDragging={dragState?.type === 'workspace' && dragState.id === workspace.workspace_id}
                                onDragStart={handleWorkspaceDragStart(workspace.workspace_id, null)}
                                onActivate={() => handleActivateWorkspace(workspace.workspace_id)}
                                onClose={() => handleCloseWorkspace(workspace.workspace_id)}
                                onMarkRead={() => handleMarkRead(workspace.workspace_id)}
                                onOpenInEditor={editors.length > 0 ? () => handleOpenInEditor(workspace.cwd) : undefined}
                                onMoveToSection={(sectionId) => handleMoveWorkspaceToSection(workspace.workspace_id, sectionId)}
                            />
                        </div>
                    {/each}
                {/if}
            </div>

            <!-- Named sections -->
            {#each sectionedWorkspaces.sectionGroups as group, sIdx (group.section.section_id)}
                <div class="section-group" style="border-left-color: {group.section.color};" data-drop-zone-section={group.section.section_id}>
                    <div data-section-header-id={group.section.section_id} data-section-idx={sIdx}>
                        <SectionHeader
                            section={group.section}
                            workspaceCount={group.workspaces.length}
                            isDragging={dragState?.type === 'section' && dragState.id === group.section.section_id}
                            onDragStart={handleSectionDragStart(group.section.section_id)}
                            onToggleCollapse={() => handleToggleSectionCollapse(group.section.section_id)}
                            onRename={(name) => handleRenameSection(group.section.section_id, name)}
                            onChangeColor={(color) => handleSetSectionColor(group.section.section_id, color)}
                            onDelete={() => handleDeleteSection(group.section.section_id)}
                        />
                    </div>
                    {#if !group.section.collapsed}
                        {#each group.workspaces as workspace, idx (workspace.workspace_id)}
                            <div data-ws-id={workspace.workspace_id} data-ws-index={idx}>
                                <WorkspaceRow
                                    {workspace}
                                    isActive={workspace.workspace_id === $appState.active_workspace_id}
                                    sections={$appState.sections}
                                    currentSectionId={group.section.section_id}
                                    isDragging={dragState?.type === 'workspace' && dragState.id === workspace.workspace_id}
                                    onDragStart={handleWorkspaceDragStart(workspace.workspace_id, group.section.section_id)}
                                    onActivate={() => handleActivateWorkspace(workspace.workspace_id)}
                                    onClose={() => handleCloseWorkspace(workspace.workspace_id)}
                                    onMarkRead={() => handleMarkRead(workspace.workspace_id)}
                                    onOpenInEditor={editors.length > 0 ? () => handleOpenInEditor(workspace.cwd) : undefined}
                                    onMoveToSection={(sectionId) => handleMoveWorkspaceToSection(workspace.workspace_id, sectionId)}
                                />
                            </div>
                        {/each}
                    {/if}
                </div>
            {/each}

            <!-- Drop indicator -->
            {#if dropIndicatorY !== null && dragState}
                <div class="drop-indicator" style="top: {dropIndicatorY}px;"></div>
            {/if}

        {:else}
            <div class="empty-workspace-hint">
                <button class="create-first-btn" type="button" onclick={handleCreateWorkspace}>
                    Create first workspace
                </button>
            </div>
        {/if}
    </div>

    <div class="sidebar-bottom">
        <!-- Secondary sections -->
        <div class="sidebar-sections">
            {#if ($appState?.detected_ports?.length ?? 0) > 0}
                <div class="sidebar-divider"></div>
                <PortsSection />
            {/if}

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
    </div>
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
        padding: 10px 12px 8px;
        flex-shrink: 0;
    }

    .brand-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }

    .brand-actions {
        display: flex;
        align-items: center;
        gap: 4px;
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
        border-radius: var(--ui-radius-sm);
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
        border-radius: var(--ui-radius-md);
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
        border-radius: var(--ui-radius-sm);
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

    .active-git-info {
        display: flex;
        align-items: center;
        gap: 4px;
        font-family: var(--ui-font-mono);
        font-size: 0.72rem;
        color: var(--ui-text-muted);
    }

    .head-diff-add { color: var(--ui-success); font-weight: 600; }
    .head-diff-del { color: var(--ui-danger); font-weight: 600; }

    .head-actions {
        display: flex;
        align-items: center;
        gap: 6px;
    }

    .head-action-btn {
        padding: 4px 9px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
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
        position: relative;
    }

    .drop-indicator {
        position: absolute;
        left: 8px;
        right: 8px;
        height: 2px;
        background: var(--ui-accent);
        border-radius: 1px;
        pointer-events: none;
        z-index: 10;
    }

    .unsorted-label {
        font-size: 0.72rem;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
        padding: 8px 12px 4px;
    }

    .section-group {
        display: flex;
        flex-direction: column;
        border-left: 2px solid transparent;
        border-radius: var(--ui-radius-sm);
        margin-top: 2px;
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
        border-radius: var(--ui-radius-md);
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

    .sidebar-bottom {
        display: flex;
        flex-direction: column;
        margin-top: auto;
        min-height: 0;
        flex-shrink: 0;
    }

    /* ---- Secondary sections ---- */

    .sidebar-sections {
        display: flex;
        flex-direction: column;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
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
        border-radius: var(--ui-radius-sm);
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
        border-radius: var(--ui-radius-sm);
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

    /* Worktree delete confirmation */
    .confirm-backdrop {
        position: fixed;
        inset: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.4);
        z-index: 200;
    }

    .confirm-dialog {
        max-width: 360px;
        padding: 16px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 16px 40px rgba(0, 0, 0, 0.4);
    }

    .confirm-title {
        margin: 0 0 8px;
        font-size: 0.88rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .confirm-text {
        margin: 0 0 12px;
        font-size: 0.8rem;
        color: var(--ui-text-secondary);
        line-height: 1.4;
    }

    .confirm-actions {
        display: flex;
        gap: 6px;
        justify-content: flex-end;
    }

    .confirm-btn {
        padding: 5px 12px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.78rem;
        cursor: pointer;
        transition: all 120ms ease-out;
    }

    .confirm-btn:hover {
        border-color: var(--ui-border-strong);
    }

    .confirm-danger {
        background: color-mix(in srgb, var(--ui-danger) 14%, var(--ui-layer-3) 86%);
        border-color: color-mix(in srgb, var(--ui-danger) 24%, transparent);
        color: var(--ui-danger);
    }

    .confirm-danger:hover {
        background: color-mix(in srgb, var(--ui-danger) 20%, var(--ui-layer-3) 80%);
    }

    .confirm-hint {
        margin: 0 0 12px;
        font-size: 0.74rem;
        color: var(--ui-text-muted);
        font-style: italic;
    }

    .teardown-error-text {
        white-space: pre-wrap;
        word-break: break-word;
        max-height: 200px;
        overflow-y: auto;
        font-family: var(--ui-font-mono);
        font-size: 0.74rem;
    }
</style>
