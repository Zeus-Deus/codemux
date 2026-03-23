<script lang="ts">
    import { onMount, onDestroy } from 'svelte';
    import { listen } from '@tauri-apps/api/event';
    import { runWorkspaceSetup } from '../../stores/workspace';
    import { showUiNotice } from '../../stores/uiNotice';

    let { workspaceId }: { workspaceId: string } = $props();

    type SetupState = 'idle' | 'running' | 'complete' | 'failed';

    let setupState = $state<SetupState>('idle');
    let currentCommand = $state('');
    let progress = $state({ index: 0, total: 0 });
    let errorInfo = $state({ command: '', stderr: '', exitCode: null as number | null });
    let unlisteners: Array<() => void> = [];

    onMount(async () => {
        unlisteners.push(
            await listen<{ workspace_id: string; command: string; index: number; total: number }>(
                'workspace-setup-progress',
                (event) => {
                    if (event.payload.workspace_id === workspaceId) {
                        setupState = 'running';
                        currentCommand = event.payload.command;
                        progress = { index: event.payload.index, total: event.payload.total };
                    }
                },
            ),
        );
        unlisteners.push(
            await listen<{ workspace_id: string }>(
                'workspace-setup-complete',
                (event) => {
                    if (event.payload.workspace_id === workspaceId) {
                        setupState = 'complete';
                        setTimeout(() => { if (setupState === 'complete') setupState = 'idle'; }, 500);
                    }
                },
            ),
        );
        unlisteners.push(
            await listen<{ workspace_id: string; command: string; stdout: string; stderr: string; exit_code: number | null }>(
                'workspace-setup-failed',
                (event) => {
                    if (event.payload.workspace_id === workspaceId) {
                        setupState = 'failed';
                        errorInfo = {
                            command: event.payload.command,
                            stderr: event.payload.stderr || event.payload.stdout,
                            exitCode: event.payload.exit_code,
                        };
                    }
                },
            ),
        );
    });

    onDestroy(() => {
        for (const unlisten of unlisteners) unlisten();
    });

    async function handleRetry() {
        setupState = 'idle';
        try {
            await runWorkspaceSetup(workspaceId);
        } catch (error) {
            showUiNotice(String(error), 'error');
        }
    }

    function handleSkip() {
        setupState = 'idle';
    }
</script>

{#if setupState === 'running'}
    <div class="setup-overlay">
        <div class="setup-card">
            <div class="setup-spinner"></div>
            <h3 class="setup-heading">Running setup...</h3>
            <p class="setup-progress">({progress.index + 1}/{progress.total})</p>
            <code class="setup-command">{currentCommand}</code>
        </div>
    </div>
{:else if setupState === 'failed'}
    <div class="setup-overlay">
        <div class="setup-card setup-card-error">
            <h3 class="setup-heading error-heading">Setup failed</h3>
            <p class="setup-error-cmd">Command: <code>{errorInfo.command}</code></p>
            {#if errorInfo.exitCode !== null}
                <p class="setup-error-exit">Exit code: {errorInfo.exitCode}</p>
            {/if}
            {#if errorInfo.stderr}
                <pre class="setup-error-output">{errorInfo.stderr}</pre>
            {/if}
            <div class="setup-actions">
                <button class="setup-btn" type="button" onclick={handleSkip}>Skip</button>
                <button class="setup-btn setup-btn-primary" type="button" onclick={handleRetry}>Retry</button>
            </div>
        </div>
    </div>
{/if}

<style>
    .setup-overlay {
        position: absolute;
        inset: 0;
        z-index: 10;
        display: flex;
        align-items: center;
        justify-content: center;
        background: var(--ui-layer-1);
    }

    .setup-card {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 10px;
        padding: 32px 40px;
        max-width: 480px;
    }

    .setup-card-error {
        align-items: flex-start;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-lg);
        padding: 20px 24px;
        width: min(480px, 90%);
    }

    .setup-spinner {
        width: 24px;
        height: 24px;
        border: 2px solid var(--ui-border-soft);
        border-top-color: var(--ui-accent);
        border-radius: 50%;
        animation: spin 0.8s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }

    .setup-heading {
        margin: 0;
        font-size: 0.92rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .error-heading {
        color: var(--ui-danger);
    }

    .setup-progress {
        margin: 0;
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        color: var(--ui-text-muted);
    }

    .setup-command {
        font-family: var(--ui-font-mono);
        font-size: 0.8rem;
        color: var(--ui-text-secondary);
        background: var(--ui-layer-2);
        padding: 6px 12px;
        border-radius: var(--ui-radius-sm);
        max-width: 400px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .setup-error-cmd {
        margin: 0;
        font-size: 0.78rem;
        color: var(--ui-text-secondary);
    }

    .setup-error-cmd code {
        font-family: var(--ui-font-mono);
        color: var(--ui-text-primary);
    }

    .setup-error-exit {
        margin: 0;
        font-size: 0.75rem;
        font-family: var(--ui-font-mono);
        color: var(--ui-text-muted);
    }

    .setup-error-output {
        margin: 4px 0 0;
        padding: 8px 10px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        font-family: var(--ui-font-mono);
        font-size: 0.74rem;
        color: var(--ui-danger);
        white-space: pre-wrap;
        word-break: break-word;
        max-height: 160px;
        overflow-y: auto;
        width: 100%;
        box-sizing: border-box;
    }

    .setup-actions {
        display: flex;
        gap: 8px;
        margin-top: 8px;
        align-self: flex-end;
    }

    .setup-btn {
        padding: 5px 14px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.78rem;
        cursor: pointer;
        transition: all 120ms ease-out;
    }

    .setup-btn:hover {
        border-color: var(--ui-border-strong);
    }

    .setup-btn-primary {
        background: color-mix(in srgb, var(--ui-accent) 14%, var(--ui-layer-3) 86%);
        border-color: color-mix(in srgb, var(--ui-accent) 24%, transparent);
    }
</style>
