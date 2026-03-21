<script lang="ts">
    import { tick } from 'svelte';
    import {
        commLogStore,
        getCommunicationLog,
        injectOrchestratorMessage,
    } from '../../stores/openflow';
    import { mergeCommLogEntries } from '../../lib/openflowPolling';

    let { runId }: { runId: string | null } = $props();

    let newMessage = $state('');
    let messagesContainer: HTMLDivElement;
    let injectError = $state<string | null>(null);
    let autoFollow = $state(true);
    let showJumpToLatest = $state(false);

    // Subscribe to the shared store — OrchestrationView owns the single polling interval.
    // Limit to last 100 messages for performance with 20+ agents.
    // Two-pass: track delivery state for user injections, then filter system markers.
    const messages = $derived.by(() => {
        const raw = $commLogStore.slice(-100);

        // Pass 1: Find which injection counts have been handled
        const handledCounts = new Set<number>();
        for (const msg of raw) {
            if (msg.role.toLowerCase() === 'system' && msg.message.startsWith('HANDLED_INJECTIONS: ')) {
                const n = parseInt(msg.message.slice('HANDLED_INJECTIONS: '.length));
                if (!isNaN(n)) handledCounts.add(n);
            }
        }

        // Pass 2: Tag user injections with delivery status, filter system markers
        let injectionIndex = 0;
        return raw.map((msg) => {
            const roleLower = msg.role.toLowerCase();
            if (roleLower.startsWith('user')) injectionIndex++;
            const delivered = roleLower.startsWith('user') && handledCounts.has(injectionIndex);
            return { ...msg, delivered };
        }).filter((msg) => {
            const roleLower = msg.role.toLowerCase();
            if (roleLower === 'system') {
                return !msg.message.startsWith('HANDLED_INJECTIONS:')
                    && !msg.message.startsWith('HANDLED_ASSIGNMENTS:')
                    && !msg.message.startsWith('DONE_RELAY_COUNT:')
                    && !msg.message.startsWith('INJECTION_PENDING:');
            }
            return true;
        });
    });

    $effect(() => {
        const _len = messages.length;
        const container = messagesContainer;
        if (!container) return;

        tick().then(() => {
            if (autoFollow || messages.length < 10) {
                container.scrollTop = container.scrollHeight;
                showJumpToLatest = false;
            }
        });
    });

    function syncScrollState() {
        const container = messagesContainer;
        if (!container) return;

        const threshold = 100;
        const nearBottom =
            container.scrollTop + container.clientHeight >= container.scrollHeight - threshold;
        autoFollow = nearBottom;
        showJumpToLatest = !nearBottom;
    }

    function jumpToLatest() {
        const container = messagesContainer;
        if (!container) return;
        container.scrollTop = container.scrollHeight;
        autoFollow = true;
        showJumpToLatest = false;
    }

    async function handleSend() {
        if (!newMessage.trim() || !runId) return;
        injectError = null;
        const currentRunId = runId;
        try {
            autoFollow = true;
            await injectOrchestratorMessage(currentRunId, newMessage.trim());
            newMessage = '';
            // The backend loop wakes automatically on injection — no need to trigger
            // the orchestrator cycle from the frontend. Just fetch the latest log.
            const entries = await getCommunicationLog(currentRunId);
            commLogStore.update(existing => mergeCommLogEntries(existing, entries));
            tick().then(jumpToLatest);
        } catch (e) {
            console.error('Failed to inject message:', e);
            injectError = String(e);
        }
    }

    function handleKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSend();
        }
    }

    function getRoleClass(role: string): string {
        if (role.startsWith('user')) return 'user';
        if (role.startsWith('orchestrator')) return 'orchestrator';
        if (role.startsWith('builder')) return 'builder';
        if (role.startsWith('reviewer')) return 'reviewer';
        if (role.startsWith('tester')) return 'tester';
        return '';
    }

    function formatRole(role: string): string {
        // Convert "orchestrator" to "Orchestrator", "user/inject" to "User"
        const parts = role.split('/');
        const main = parts[0];
        return main.charAt(0).toUpperCase() + main.slice(1);
    }
</script>

<div class="communication-panel">
    <div class="panel-header">
        <h3>Communication</h3>
        <div class="panel-header-actions">
            {#if showJumpToLatest}
                <button class="jump-btn" type="button" onclick={jumpToLatest}>Latest</button>
            {/if}
            <span class="badge">Live</span>
        </div>
    </div>

    <div class="messages" bind:this={messagesContainer} onscroll={syncScrollState}>
        {#if messages.length > 0}
            {#each messages as msg}
                <div class="message {getRoleClass(msg.role)}">
                    <span class="message-role">
                        {formatRole(msg.role)}
                        {#if msg.delivered}
                            <span class="delivered-badge">Delivered</span>
                        {/if}
                    </span>
                    <span class="message-text">{msg.message}</span>
                </div>
            {/each}
        {:else}
            <p class="no-messages">No messages yet. Agents will communicate here.</p>
        {/if}
    </div>

    <div class="inject-form">
        {#if injectError}
            <div class="inject-error">{injectError}</div>
        {/if}
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

    .panel-header-actions {
        display: flex;
        align-items: center;
        gap: 8px;
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

    .jump-btn {
        padding: 6px 10px;
        border-radius: 999px;
        border: 1px solid color-mix(in srgb, var(--ui-accent) 30%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.72rem;
        font-weight: 600;
        cursor: pointer;
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

    .message-role {
        display: block;
        font-size: 0.7rem;
        font-weight: 600;
        color: var(--ui-accent);
        margin-bottom: 4px;
        text-transform: uppercase;
    }

    .message.user .message-role {
        color: var(--ui-accent);
    }

    .message.orchestrator .message-role {
        color: var(--ui-success);
    }

    .message.builder .message-role {
        color: var(--ui-attention);
    }

    .message.reviewer .message-role {
        color: #a78bfa;
    }

    .message.tester .message-role {
        color: #34d399;
    }

    .delivered-badge {
        display: inline-block;
        margin-left: 6px;
        padding: 1px 6px;
        background: color-mix(in srgb, var(--ui-success) 20%, transparent);
        border-radius: 4px;
        font-size: 0.6rem;
        font-weight: 600;
        color: var(--ui-success);
        text-transform: uppercase;
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
        font-family: var(--ui-font-mono);
    }

    .no-messages {
        color: var(--ui-text-muted);
        font-size: 0.85rem;
        text-align: center;
        margin: auto;
    }

    .inject-form {
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 12px;
        border-top: 1px solid var(--ui-border-soft);
    }

    .inject-error {
        padding: 8px;
        background: color-mix(in srgb, var(--ui-danger) 15%, transparent);
        border: 1px solid var(--ui-danger);
        border-radius: 6px;
        color: var(--ui-danger);
        font-size: 0.8rem;
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
