<script lang="ts">
    import { appState, markWorkspaceNotificationsRead } from '../../stores/appState';

    let { workspaceId }: { workspaceId: string } = $props();

    let expanded = $state(false);

    const notifications = $derived(
        $appState?.notifications.filter((n) => n.workspace_id === workspaceId) ?? []
    );

    const unreadCount = $derived(notifications.filter((n) => !n.read).length);

    $effect(() => {
        if (unreadCount > 0) expanded = true;
    });

    function formatTime(ms: number) {
        return new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }

    async function handleMarkRead() {
        try {
            await markWorkspaceNotificationsRead(workspaceId);
        } catch (error) {
            console.error('Failed to mark notifications read:', error);
        }
    }
</script>

<div class="section">
    <button
        class="section-header"
        type="button"
        onclick={() => (expanded = !expanded)}
    >
        <span class="section-label">Alerts</span>
        {#if unreadCount > 0}
            <span class="unread-badge">{unreadCount}</span>
        {/if}
        <span class="spacer"></span>
        <span class="chevron" class:open={expanded}>›</span>
    </button>

    {#if expanded}
        <div class="section-body">
            {#if notifications.length > 0}
                <div class="notif-list">
                    {#each notifications as notif (notif.notification_id)}
                        <div
                            class="notif-item"
                            class:unread={!notif.read}
                            class:attention={notif.level === 'attention'}
                        >
                            <span class="notif-dot" class:attention={notif.level === 'attention'}></span>
                            <span class="notif-msg">{notif.message}</span>
                            <span class="notif-time">{formatTime(notif.created_at_ms)}</span>
                        </div>
                    {/each}
                </div>
                {#if unreadCount > 0}
                    <button class="mark-read-btn" type="button" onclick={handleMarkRead}>
                        Mark all read
                    </button>
                {/if}
            {:else}
                <p class="empty-hint">No alerts for this workspace.</p>
            {/if}
        </div>
    {/if}
</div>

<style>
    .section {
        display: flex;
        flex-direction: column;
    }

    .section-header {
        display: flex;
        align-items: center;
        gap: 7px;
        padding: 8px 12px;
        background: transparent;
        border: none;
        color: var(--ui-text-secondary);
        font: inherit;
        font-size: 0.76rem;
        font-weight: 600;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        cursor: pointer;
        border-radius: 6px;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
        text-align: left;
        width: 100%;
    }

    .section-header:hover {
        background: color-mix(in srgb, var(--ui-accent) 6%, transparent);
        color: var(--ui-text-primary);
    }

    .section-label {
        flex-shrink: 0;
    }

    .unread-badge {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-width: 18px;
        height: 18px;
        padding: 0 5px;
        border-radius: 9px;
        background: var(--ui-attention);
        color: #0d1117;
        font-size: 0.66rem;
        font-weight: 700;
        flex-shrink: 0;
    }

    .spacer {
        flex: 1;
    }

    .chevron {
        font-size: 1rem;
        line-height: 1;
        color: var(--ui-text-muted);
        transition: transform var(--ui-motion-fast);
        display: inline-block;
        flex-shrink: 0;
    }

    .chevron.open {
        transform: rotate(90deg);
    }

    .section-body {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 2px 8px 8px;
    }

    .notif-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .notif-item {
        display: flex;
        align-items: baseline;
        gap: 7px;
        padding: 6px 8px;
        border-radius: 5px;
        background: transparent;
        transition: background var(--ui-motion-fast);
    }

    .notif-item.unread {
        background: color-mix(in srgb, var(--ui-attention) 6%, var(--ui-layer-1) 94%);
    }

    .notif-item.unread.attention {
        background: color-mix(in srgb, var(--ui-attention) 9%, var(--ui-layer-1) 91%);
    }

    .notif-dot {
        width: 5px;
        height: 5px;
        border-radius: 50%;
        background: var(--ui-text-muted);
        flex: 0 0 5px;
        margin-top: 2px;
    }

    .notif-dot.attention {
        background: var(--ui-attention);
    }

    .notif-msg {
        flex: 1;
        font-size: 0.78rem;
        color: var(--ui-text-secondary);
        line-height: 1.4;
        min-width: 0;
        overflow: hidden;
        display: -webkit-box;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        -webkit-box-orient: vertical;
    }

    .notif-item.unread .notif-msg {
        color: var(--ui-text-primary);
    }

    .notif-time {
        font-size: 0.68rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        flex-shrink: 0;
        margin-left: auto;
    }

    .mark-read-btn {
        align-self: flex-start;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.74rem;
        padding: 4px 8px;
        cursor: pointer;
        margin-top: 4px;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .mark-read-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .empty-hint {
        margin: 0;
        font-size: 0.76rem;
        color: var(--ui-text-muted);
        padding: 4px 8px;
        line-height: 1.4;
    }
</style>
