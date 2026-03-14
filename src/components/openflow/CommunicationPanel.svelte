<script lang="ts">
    import { openflowRuntime } from '../../stores/appState';

    let { runId }: { runId: string | null } = $props();

    let newMessage = $state('');
    let messagesContainer: HTMLDivElement;

    const run = $derived(
        runId ? $openflowRuntime?.active_runs.find(r => r.run_id === runId) ?? null : null
    );

    const messages = $derived(
        run?.timeline.map(entry => ({
            role: entry.level,
            message: entry.message,
            timestamp: entry.entry_id,
            type: entry.level
        })) ?? []
    );

    function getMessageClass(msg: { type: string }): string {
        if (msg.type === 'warning') return 'warning';
        if (msg.type === 'error') return 'error';
        return '';
    }

    function handleSend() {
        // TODO: Send message to orchestrator
        // This will be implemented in Phase 3
        console.log('User message:', newMessage);
        newMessage = '';
    }

    function handleKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSend();
        }
    }
</script>

<div class="communication-panel">
    <div class="panel-header">
        <h3>Communication</h3>
        <span class="badge">Live</span>
    </div>

    <div class="messages" bind:this={messagesContainer}>
        {#if messages.length > 0}
            {#each messages as msg}
                <div class="message {getMessageClass(msg)}">
                    <span class="message-text">{msg.message}</span>
                </div>
            {/each}
        {:else}
            <p class="no-messages">No messages yet</p>
        {/if}
    </div>

    <div class="inject-form">
        <input
            type="text"
            bind:value={newMessage}
            placeholder="Inject to orchestrator..."
            onkeydown={handleKeydown}
        />
        <button type="button" onclick={handleSend} disabled={!newMessage.trim()}>
            Send
        </button>
    </div>
</div>

<style>
    .communication-panel {
        display: flex;
        flex-direction: column;
        background: var(--ui-layer-1);
        border-left: 1px solid var(--ui-border-soft);
        height: 100%;
    }

    .panel-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 16px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .panel-header h3 {
        margin: 0;
        font-size: 0.9rem;
        font-weight: 600;
        color: var(--ui-text-primary);
    }

    .badge {
        padding: 2px 8px;
        background: color-mix(in srgb, var(--ui-success) 15%, transparent);
        border: 1px solid color-mix(in srgb, var(--ui-success) 30%, transparent);
        border-radius: 10px;
        font-size: 0.65rem;
        font-weight: 600;
        color: var(--ui-success);
        text-transform: uppercase;
    }

    .messages {
        flex: 1;
        overflow-y: auto;
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .message {
        padding: 10px 12px;
        background: var(--ui-layer-2);
        border-radius: 8px;
        font-size: 0.85rem;
    }

    .message.user {
        background: color-mix(in srgb, var(--ui-accent) 15%, var(--ui-layer-2));
        border-left: 3px solid var(--ui-accent);
    }

    .message.warning {
        background: color-mix(in srgb, var(--ui-attention) 10%, var(--ui-layer-2));
    }

    .message.error {
        background: color-mix(in srgb, var(--ui-danger) 10%, var(--ui-layer-2));
    }

    .message-text {
        color: var(--ui-text-secondary);
        word-break: break-word;
    }

    .no-messages {
        color: var(--ui-text-muted);
        font-size: 0.85rem;
        text-align: center;
        margin: auto;
    }

    .inject-form {
        display: flex;
        gap: 8px;
        padding: 12px;
        border-top: 1px solid var(--ui-border-soft);
    }

    .inject-form input {
        flex: 1;
        padding: 10px 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.85rem;
    }

    .inject-form input:focus {
        outline: none;
        border-color: var(--ui-accent);
    }

    .inject-form button {
        padding: 10px 16px;
        background: var(--ui-accent);
        border: none;
        border-radius: 6px;
        color: #fff;
        font: inherit;
        font-size: 0.85rem;
        font-weight: 600;
        cursor: pointer;
    }

    .inject-form button:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }
</style>
