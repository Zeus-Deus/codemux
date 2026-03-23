<script lang="ts">
    import { onMount } from 'svelte';
    import DiffContent from '../diff/DiffContent.svelte';
    import { getGitStatus, getGitDiff, getGitBranchInfo, stageFiles, unstageFiles, commitChanges, pushChanges } from '../../stores/git';
    import type { GitFileStatus, GitBranchInfo } from '../../stores/types';

    let { workspaceCwd, onClose }: { workspaceCwd: string; onClose: () => void } = $props();

    let files = $state<GitFileStatus[]>([]);
    let branchInfo = $state<GitBranchInfo>({ branch: null, ahead: 0, behind: 0 });
    let expandedFile = $state<{ path: string; staged: boolean } | null>(null);
    let expandedDiffText = $state('');
    let commitMessage = $state('');
    let isLoading = $state(false);
    let isNotGitRepo = $state(false);

    let refreshTimer: ReturnType<typeof setInterval> | null = null;

    const stagedFiles = $derived(files.filter(f => f.is_staged === true));
    const unstagedFiles = $derived(files.filter(f => f.is_unstaged !== false));

    async function refresh() {
        try {
            const [statusResult, branchResult] = await Promise.all([
                getGitStatus(workspaceCwd),
                getGitBranchInfo(workspaceCwd),
            ]);
            files = statusResult;
            branchInfo = branchResult;
            isNotGitRepo = false;

            if (expandedFile && !files.some(f => f.path === expandedFile!.path)) {
                expandedFile = null;
                expandedDiffText = '';
            }
        } catch {
            isNotGitRepo = true;
            files = [];
        }
    }

    async function loadDiff(path: string, staged: boolean) {
        try {
            expandedDiffText = await getGitDiff(workspaceCwd, path, staged);
        } catch {
            expandedDiffText = '';
        }
    }

    async function handleToggleFile(path: string, staged: boolean) {
        if (expandedFile?.path === path && expandedFile?.staged === staged) {
            expandedFile = null;
            expandedDiffText = '';
        } else {
            expandedFile = { path, staged };
            await loadDiff(path, staged);
        }
    }

    async function handleStage(filePaths: string[]) {
        await stageFiles(workspaceCwd, filePaths);
        await refresh();
        if (expandedFile) await loadDiff(expandedFile.path, expandedFile.staged);
    }

    async function handleUnstage(filePaths: string[]) {
        await unstageFiles(workspaceCwd, filePaths);
        await refresh();
        if (expandedFile) await loadDiff(expandedFile.path, expandedFile.staged);
    }

    async function handleCommit() {
        if (!commitMessage.trim()) return;
        isLoading = true;
        try {
            await commitChanges(workspaceCwd, commitMessage.trim());
            commitMessage = '';
            expandedFile = null;
            expandedDiffText = '';
            await refresh();
        } catch (e) {
            console.error('commit failed:', e);
        }
        isLoading = false;
    }

    async function handlePush() {
        isLoading = true;
        try {
            await pushChanges(workspaceCwd);
            await refresh();
        } catch (e) {
            console.error('push failed:', e);
        }
        isLoading = false;
    }

    function handleCommitKeydown(event: KeyboardEvent) {
        if (event.key === 'Enter' && !event.shiftKey) {
            event.preventDefault();
            void handleCommit();
        }
    }

    function statusLetter(status: string): string {
        switch (status) {
            case 'added': return 'A';
            case 'modified': return 'M';
            case 'deleted': return 'D';
            case 'renamed': return 'R';
            case 'untracked': return 'U';
            case 'copied': return 'C';
            default: return '?';
        }
    }

    function statusClass(status: string): string {
        switch (status) {
            case 'added': return 'status-added';
            case 'modified': return 'status-modified';
            case 'deleted': return 'status-deleted';
            case 'renamed': return 'status-renamed';
            default: return 'status-muted';
        }
    }

    function fileName(path: string): string {
        const parts = path.split('/');
        return parts[parts.length - 1];
    }

    function handleActionClick(event: MouseEvent, action: 'stage' | 'unstage', path: string) {
        event.stopPropagation();
        if (action === 'stage') void handleStage([path]);
        else void handleUnstage([path]);
    }

    onMount(() => {
        void refresh();
        refreshTimer = setInterval(() => void refresh(), 3000);
        return () => {
            if (refreshTimer) clearInterval(refreshTimer);
        };
    });
</script>

<div class="changes-panel">
    <!-- Header -->
    <div class="panel-header">
        <span class="panel-title">Changes</span>
        {#if files.length > 0}
            <span class="file-count-badge">{files.length}</span>
        {/if}
        <div class="header-spacer"></div>
        <button class="header-btn" onclick={() => void refresh()} title="Refresh" aria-label="Refresh">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M21 2v6h-6M3 22v-6h6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                <path d="M3 12a9 9 0 0115.36-6.36L21 8M21 12a9 9 0 01-15.36 6.36L3 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
        </button>
        <button class="header-btn close-btn" onclick={onClose} title="Close panel" aria-label="Close changes panel">
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
            </svg>
        </button>
    </div>

    {#if isNotGitRepo}
        <div class="panel-empty">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" stroke="currentColor" stroke-width="1.5"/>
            </svg>
            <span>Not a git repository</span>
        </div>
    {:else}
        <!-- Commit controls -->
        <div class="commit-controls">
            <input
                class="commit-input"
                type="text"
                placeholder="Commit message..."
                bind:value={commitMessage}
                onkeydown={handleCommitKeydown}
            />
            <div class="commit-actions">
                <button
                    class="commit-btn"
                    onclick={() => void handleCommit()}
                    disabled={!commitMessage.trim() || !stagedFiles.length || isLoading}
                >
                    Commit
                </button>
                {#if branchInfo.ahead > 0}
                    <button
                        class="push-btn"
                        onclick={() => void handlePush()}
                        disabled={isLoading}
                    >
                        Push ↑{branchInfo.ahead}
                    </button>
                {/if}
            </div>
        </div>

        <!-- File sections -->
        <div class="file-sections">
            <!-- Staged section -->
            {#if stagedFiles.length > 0}
                <div class="section">
                    <div class="section-header">
                        <span class="section-label">Staged</span>
                        <span class="section-count">{stagedFiles.length}</span>
                        <button class="section-action" onclick={() => void handleUnstage(stagedFiles.map(f => f.path))}>
                            Unstage All
                        </button>
                    </div>
                    {#each stagedFiles as file (file.path + ':staged')}
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <div
                            class="file-row"
                            class:expanded={expandedFile?.path === file.path && expandedFile?.staged === true}
                            onclick={() => void handleToggleFile(file.path, true)}
                            onkeydown={(e) => { if (e.key === 'Enter') void handleToggleFile(file.path, true); }}
                            role="option"
                            tabindex="0"
                            aria-selected={expandedFile?.path === file.path && expandedFile?.staged === true}
                            title={file.path}
                        >
                            <span class="status-badge {statusClass(file.status)}">{statusLetter(file.status)}</span>
                            <span class="file-name">{fileName(file.path)}</span>
                            <button
                                class="action-btn unstage-btn"
                                onclick={(e) => handleActionClick(e, 'unstage', file.path)}
                                title="Unstage"
                            >
                                <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                    <path d="M1.5 5h7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                                </svg>
                            </button>
                        </div>
                        {#if expandedFile?.path === file.path && expandedFile?.staged === true}
                            <div class="inline-diff">
                                <DiffContent diffText={expandedDiffText} filePath={file.path} />
                            </div>
                        {/if}
                    {/each}
                </div>
            {/if}

            <!-- Unstaged section -->
            <div class="section">
                <div class="section-header">
                    <span class="section-label">Changes</span>
                    <span class="section-count">{unstagedFiles.length}</span>
                    {#if unstagedFiles.length > 0}
                        <button class="section-action" onclick={() => void handleStage(unstagedFiles.map(f => f.path))}>
                            Stage All
                        </button>
                    {/if}
                </div>
                {#each unstagedFiles as file (file.path + ':unstaged')}
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div
                        class="file-row"
                        class:expanded={expandedFile?.path === file.path && expandedFile?.staged === false}
                        onclick={() => void handleToggleFile(file.path, false)}
                        onkeydown={(e) => { if (e.key === 'Enter') void handleToggleFile(file.path, false); }}
                        role="option"
                        tabindex="0"
                        aria-selected={expandedFile?.path === file.path && expandedFile?.staged === false}
                        title={file.path}
                    >
                        <span class="status-badge {statusClass(file.status)}">{statusLetter(file.status)}</span>
                        <span class="file-name">{fileName(file.path)}</span>
                        <button
                            class="action-btn stage-btn"
                            onclick={(e) => handleActionClick(e, 'stage', file.path)}
                            title="Stage"
                        >
                            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                <path d="M5 1.5v7M1.5 5h7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                            </svg>
                        </button>
                    </div>
                    {#if expandedFile?.path === file.path && expandedFile?.staged === false}
                        <div class="inline-diff">
                            <DiffContent diffText={expandedDiffText} filePath={file.path} />
                        </div>
                    {/if}
                {/each}
            </div>

            {#if stagedFiles.length === 0 && unstagedFiles.length === 0}
                <div class="panel-empty">
                    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                        <path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    </svg>
                    <span>Working tree clean</span>
                </div>
            {/if}
        </div>
    {/if}
</div>

<style>
    .changes-panel {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        background: var(--ui-layer-1);
        border-left: 1px solid var(--ui-border-soft);
    }

    /* Header */
    .panel-header {
        display: flex;
        align-items: center;
        gap: 6px;
        height: 40px;
        min-height: 40px;
        padding: 0 12px;
        border-bottom: 1px solid var(--ui-border-soft);
        flex-shrink: 0;
    }

    .panel-title {
        font-size: 0.8rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .file-count-badge {
        font-size: 0.72rem;
        font-weight: 600;
        color: var(--ui-text-muted);
        background: var(--ui-layer-2);
        padding: 1px 6px;
        border-radius: var(--ui-radius-md);
    }

    .header-spacer {
        flex: 1;
    }

    .header-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 24px;
        height: 24px;
        padding: 0;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        flex-shrink: 0;
        transition: all var(--ui-motion-fast);
    }

    .header-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .close-btn:hover {
        background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
        color: var(--ui-danger);
    }

    /* Commit controls */
    .commit-controls {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px 12px;
        border-bottom: 1px solid var(--ui-border-soft);
        flex-shrink: 0;
    }

    .commit-input {
        width: 100%;
        height: 32px;
        padding: 0 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        outline: none;
        box-sizing: border-box;
        box-shadow: var(--ui-shadow-xs);
        transition: border-color var(--ui-motion-fast), box-shadow var(--ui-motion-fast);
    }

    .commit-input:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 50%, transparent);
        box-shadow: 0 0 0 3px var(--ui-ring-color);
    }

    .commit-input::placeholder {
        color: var(--ui-text-muted);
    }

    .commit-actions {
        display: flex;
        gap: 6px;
    }

    .commit-btn {
        flex: 1;
        height: 32px;
        padding: 0 12px;
        background: var(--ui-accent);
        border: none;
        border-radius: var(--ui-radius-sm);
        color: var(--ui-layer-0);
        font-size: 0.75rem;
        font-weight: 600;
        cursor: pointer;
        box-shadow: var(--ui-shadow-xs);
        transition: opacity var(--ui-motion-fast), box-shadow var(--ui-motion-fast);
    }

    .commit-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .commit-btn:not(:disabled):hover {
        opacity: 0.9;
    }

    .commit-btn:focus-visible {
        outline: none;
        box-shadow: 0 0 0 3px var(--ui-ring-color);
    }

    .push-btn {
        height: 32px;
        padding: 0 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
        font-family: var(--ui-font-mono);
        font-size: 0.72rem;
        cursor: pointer;
        flex-shrink: 0;
        transition: all var(--ui-motion-fast);
    }

    .push-btn:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    .push-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    /* File sections */
    .file-sections {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
    }

    .section {
        margin-bottom: 4px;
    }

    .section-header {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 8px 12px 4px;
        flex-shrink: 0;
    }

    .section-label {
        font-size: 0.75rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
    }

    .section-count {
        font-size: 0.68rem;
        font-weight: 600;
        color: var(--ui-text-muted);
        background: var(--ui-layer-2);
        padding: 1px 6px;
        border-radius: var(--ui-radius-md);
    }

    .section-action {
        margin-left: auto;
        padding: 2px 8px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        font-size: 0.72rem;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
    }

    .section-action:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    /* File rows */
    .file-row {
        display: flex;
        align-items: center;
        gap: 8px;
        height: 28px;
        padding: 0 12px;
        cursor: pointer;
        transition: background var(--ui-motion-fast);
    }

    .file-row:hover {
        background: color-mix(in srgb, var(--ui-layer-2) 50%, transparent);
    }

    .file-row.expanded {
        background: var(--ui-layer-2);
    }

    .status-badge {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 16px;
        height: 16px;
        border-radius: var(--ui-radius-sm);
        font-family: var(--ui-font-mono);
        font-size: 0.65rem;
        font-weight: 700;
        flex-shrink: 0;
    }

    .status-added { color: var(--ui-success); background: color-mix(in srgb, var(--ui-success) 15%, transparent); }
    .status-modified { color: var(--ui-attention); background: color-mix(in srgb, var(--ui-attention) 15%, transparent); }
    .status-deleted { color: var(--ui-danger); background: color-mix(in srgb, var(--ui-danger) 15%, transparent); }
    .status-renamed { color: var(--ui-accent); background: color-mix(in srgb, var(--ui-accent) 15%, transparent); }
    .status-muted { color: var(--ui-text-muted); background: var(--ui-layer-2); }

    .file-name {
        flex: 1;
        min-width: 0;
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .action-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 20px;
        height: 20px;
        padding: 0;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        flex-shrink: 0;
        opacity: 0;
        transition: all var(--ui-motion-fast);
    }

    .file-row:hover .action-btn {
        opacity: 1;
    }

    .stage-btn:hover {
        background: color-mix(in srgb, var(--ui-success) 15%, transparent);
        color: var(--ui-success);
    }

    .unstage-btn:hover {
        background: color-mix(in srgb, var(--ui-danger) 15%, transparent);
        color: var(--ui-danger);
    }

    /* Inline diff */
    .inline-diff {
        max-height: 300px;
        overflow-y: auto;
        border-top: 1px solid var(--ui-border-soft);
        border-bottom: 1px solid var(--ui-border-soft);
        background: var(--ui-layer-0);
    }

    /* Empty state */
    .panel-empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 8px;
        padding: 32px 16px;
        color: var(--ui-text-muted);
        font-size: 0.78rem;
        flex: 1;
    }
</style>
