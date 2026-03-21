<script lang="ts">
    import { onMount } from 'svelte';
    import FileList from './FileList.svelte';
    import DiffContent from './DiffContent.svelte';
    import { getGitStatus, getGitDiff, getGitBranchInfo, stageFiles, commitChanges, pushChanges } from '../../stores/git';
    import type { GitFileStatus, GitBranchInfo } from '../../stores/types';

    let { workspaceCwd }: { workspaceCwd: string } = $props();

    let files = $state<GitFileStatus[]>([]);
    let branchInfo = $state<GitBranchInfo>({ branch: null, ahead: 0, behind: 0 });
    let selectedFile = $state<string | null>(null);
    let diffText = $state('');
    let commitMessage = $state('');
    let isNotGitRepo = $state(false);
    let isLoading = $state(false);
    let listWidth = $state(240);
    let isDragging = $state(false);

    let refreshTimer: ReturnType<typeof setInterval> | null = null;

    async function refresh() {
        try {
            const [statusResult, branchResult] = await Promise.all([
                getGitStatus(workspaceCwd),
                getGitBranchInfo(workspaceCwd),
            ]);
            files = statusResult;
            branchInfo = branchResult;
            isNotGitRepo = false;

            if (selectedFile && !files.some(f => f.path === selectedFile)) {
                selectedFile = null;
                diffText = '';
            }
        } catch {
            isNotGitRepo = true;
            files = [];
        }
    }

    async function loadDiff(path: string) {
        try {
            diffText = await getGitDiff(workspaceCwd, path, false);
        } catch {
            diffText = '';
        }
    }

    async function handleSelectFile(path: string) {
        selectedFile = path;
        await loadDiff(path);
    }

    async function handleStage(filePaths: string[]) {
        await stageFiles(workspaceCwd, filePaths);
        await refresh();
    }

    async function handleCommit() {
        if (!commitMessage.trim()) return;
        isLoading = true;
        try {
            await commitChanges(workspaceCwd, commitMessage.trim());
            commitMessage = '';
            selectedFile = null;
            diffText = '';
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

    // Resizer
    function startResize(event: MouseEvent) {
        isDragging = true;
        const onMove = (e: MouseEvent) => {
            const containerLeft = (event.target as HTMLElement).closest('.diff-view')?.getBoundingClientRect().left ?? 0;
            listWidth = Math.max(160, Math.min(400, e.clientX - containerLeft));
        };
        const onUp = () => {
            isDragging = false;
            window.removeEventListener('pointermove', onMove);
            window.removeEventListener('pointerup', onUp);
        };
        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', onUp);
    }

    onMount(() => {
        void refresh();
        refreshTimer = setInterval(() => void refresh(), 3000);
        return () => {
            if (refreshTimer) clearInterval(refreshTimer);
        };
    });
</script>

{#if isNotGitRepo}
    <div class="diff-view-empty">
        <div class="empty-icon">
            <svg width="28" height="28" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" stroke="currentColor" stroke-width="1.5"/>
            </svg>
        </div>
        <h2>Not a git repository</h2>
        <p>This workspace is not inside a git repository</p>
    </div>
{:else}
    <div class="diff-view">
        <div class="diff-toolbar">
            <button class="toolbar-btn" onclick={() => void refresh()} title="Refresh" aria-label="Refresh">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M21 2v6h-6M3 22v-6h6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    <path d="M3 12a9 9 0 0115.36-6.36L21 8M21 12a9 9 0 01-15.36 6.36L3 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>

            {#if files.length > 0}
                <span class="stat-summary">
                    <span class="stat-files">{files.length} file{files.length !== 1 ? 's' : ''}</span>
                </span>
            {/if}

            <div class="toolbar-spacer"></div>

            <input
                class="commit-input"
                type="text"
                placeholder="Commit message..."
                bind:value={commitMessage}
                onkeydown={handleCommitKeydown}
            />
            <button
                class="commit-btn"
                onclick={() => void handleCommit()}
                disabled={!commitMessage.trim() || files.length === 0 || isLoading}
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

        <div class="diff-panels">
            <div class="file-list-panel" style="width: {listWidth}px">
                <FileList
                    {files}
                    {selectedFile}
                    on:select={(e) => void handleSelectFile(e.detail.path)}
                    on:stage={(e) => void handleStage(e.detail.files)}
                />
            </div>

            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="panel-resizer"
                class:active={isDragging}
                onmousedown={startResize}
            ></div>

            <div class="diff-content-panel">
                <DiffContent {diffText} filePath={selectedFile} />
            </div>
        </div>
    </div>
{/if}

<style>
    .diff-view {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .diff-view-empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        width: 100%;
        height: 100%;
        gap: 10px;
        text-align: center;
    }

    .diff-view-empty .empty-icon {
        width: 56px;
        height: 56px;
        border-radius: 14px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--ui-text-muted);
    }

    .diff-view-empty h2 {
        margin: 0;
        font-size: 1rem;
        font-weight: 600;
        color: var(--ui-text-secondary);
    }

    .diff-view-empty p {
        margin: 0;
        font-size: 0.82rem;
        color: var(--ui-text-muted);
    }

    .diff-toolbar {
        display: flex;
        align-items: center;
        gap: 8px;
        height: 40px;
        min-height: 40px;
        padding: 0 12px;
        background: var(--ui-layer-1);
        border-bottom: 1px solid var(--ui-border-soft);
        flex-shrink: 0;
    }

    .toolbar-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 28px;
        height: 28px;
        padding: 0;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-muted);
        cursor: pointer;
        flex-shrink: 0;
        transition: all var(--ui-motion-fast);
    }

    .toolbar-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .stat-summary {
        font-size: 0.75rem;
        color: var(--ui-text-muted);
    }

    .stat-files {
        color: var(--ui-text-secondary);
    }

    .toolbar-spacer {
        flex: 1;
    }

    .commit-input {
        width: 280px;
        max-width: 360px;
        height: 28px;
        padding: 0 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-family: var(--ui-font-mono);
        font-size: 0.78rem;
        outline: none;
        transition: border-color var(--ui-motion-fast);
    }

    .commit-input:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
    }

    .commit-input::placeholder {
        color: var(--ui-text-muted);
    }

    .commit-btn {
        height: 28px;
        padding: 0 14px;
        background: var(--ui-accent);
        border: none;
        border-radius: var(--ui-radius-sm);
        color: var(--ui-layer-0);
        font-size: 0.78rem;
        font-weight: 600;
        cursor: pointer;
        flex-shrink: 0;
        transition: opacity var(--ui-motion-fast);
    }

    .commit-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .commit-btn:not(:disabled):hover {
        opacity: 0.9;
    }

    .push-btn {
        height: 28px;
        padding: 0 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
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

    .diff-panels {
        display: flex;
        flex: 1;
        min-height: 0;
        overflow: hidden;
    }

    .file-list-panel {
        flex-shrink: 0;
        overflow: hidden;
    }

    .panel-resizer {
        width: 4px;
        background: var(--ui-border-soft);
        cursor: col-resize;
        transition: background var(--ui-motion-fast);
        flex-shrink: 0;
    }

    .panel-resizer:hover,
    .panel-resizer.active {
        background: var(--ui-accent);
    }

    .diff-content-panel {
        flex: 1;
        min-width: 0;
        overflow: hidden;
    }
</style>
