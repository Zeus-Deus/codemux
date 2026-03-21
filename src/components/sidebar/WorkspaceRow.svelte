<script lang="ts">
    import type { WorkspaceSnapshot } from '../../stores/types';

    let {
        workspace,
        isActive,
        onActivate,
        onClose,
        onMarkRead
    }: {
        workspace: WorkspaceSnapshot;
        isActive: boolean;
        onActivate: () => void;
        onClose: () => void;
        onMarkRead: () => void;
    } = $props();

    function compactPath(path: string) {
        const parts = path.split(/[\\/]/).filter(Boolean);
        if (parts.length <= 2) return `/${parts.join('/')}`;
        return `~/${parts.slice(-2).join('/')}`;
    }
</script>

<div
    class="workspace-row"
    class:active={isActive}
    class:has-attention={workspace.notification_count > 0}
    role="button"
    tabindex="0"
    onclick={onActivate}
    onkeydown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onActivate();
        }
    }}
>
    <div class="row-accent"></div>

    <div class="row-body">
        <div class="row-top">
            <span
                class="row-dot"
                class:dot-active={isActive && workspace.notification_count === 0}
                class:dot-attention={workspace.notification_count > 0}
            ></span>
            <span class="row-name">{workspace.title}</span>
            {#if workspace.notification_count > 0}
                <button
                    class="attention-badge"
                    type="button"
                    title="Mark notifications read"
                    onclick={(e) => { e.stopPropagation(); onMarkRead(); }}
                >
                    {workspace.notification_count}
                </button>
            {/if}
        </div>

        <div class="row-meta">
            {#if workspace.git_branch}
                <span class="meta-branch">{workspace.git_branch}</span>
                <span class="meta-sep">·</span>
            {/if}
            <span class="meta-path">{compactPath(workspace.cwd)}</span>
        </div>
    </div>

    <button
        class="row-close"
        type="button"
        aria-label="Close workspace"
        title="Close workspace"
        onclick={(e) => { e.stopPropagation(); onClose(); }}
    >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
        </svg>
    </button>
</div>

<style>
    .workspace-row {
        position: relative;
        display: flex;
        align-items: stretch;
        border-radius: 6px;
        cursor: pointer;
        overflow: hidden;
        transition:
            background var(--ui-motion-fast),
            box-shadow var(--ui-motion-fast);
        outline: none;
        min-height: 50px;
        user-select: none;
    }

    .workspace-row:hover {
        background: color-mix(in srgb, var(--ui-accent) 6%, var(--ui-layer-1) 94%);
    }

    .workspace-row.active {
        background: color-mix(in srgb, var(--ui-accent) 10%, var(--ui-layer-1) 90%);
    }

    .workspace-row.has-attention {
        background: color-mix(in srgb, var(--ui-attention) 6%, var(--ui-layer-1) 94%);
    }

    .workspace-row:focus-visible {
        box-shadow: 0 0 0 2px var(--ui-accent);
    }

    .row-accent {
        width: 2px;
        flex: 0 0 2px;
        background: transparent;
        transition: background var(--ui-motion-fast);
        border-radius: 2px 0 0 2px;
    }

    .workspace-row.active .row-accent {
        background: var(--ui-accent);
    }

    .workspace-row.has-attention .row-accent {
        background: var(--ui-attention);
    }

    .row-body {
        flex: 1;
        min-width: 0;
        padding: 10px 6px 10px 10px;
        display: flex;
        flex-direction: column;
        gap: 3px;
    }

    .row-top {
        display: flex;
        align-items: center;
        gap: 7px;
        min-width: 0;
    }

    .row-dot {
        width: 6px;
        height: 6px;
        border-radius: 50%;
        flex: 0 0 6px;
        background: var(--ui-text-muted);
        transition: background var(--ui-motion-fast);
    }

    .row-dot.dot-active {
        background: var(--ui-accent);
        box-shadow: 0 0 6px color-mix(in srgb, var(--ui-accent) 70%, transparent);
    }

    .row-dot.dot-attention {
        background: var(--ui-attention);
    }

    .row-name {
        font-size: 0.84rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        flex: 1;
        min-width: 0;
    }

    .workspace-row:not(.active) .row-name {
        color: var(--ui-text-secondary);
        font-weight: 500;
    }

    .attention-badge {
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
        font-family: inherit;
        flex: 0 0 auto;
        border: none;
        cursor: pointer;
        transition: opacity var(--ui-motion-fast);
    }

    .attention-badge:hover {
        opacity: 0.8;
    }

    .row-meta {
        display: flex;
        align-items: center;
        gap: 5px;
        padding-left: 13px;
        min-width: 0;
        font-family: var(--ui-font-mono);
    }

    .meta-branch {
        font-size: 0.72rem;
        color: var(--ui-text-secondary);
        white-space: nowrap;
        flex-shrink: 0;
    }

    .meta-sep {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        flex-shrink: 0;
    }

    .meta-path {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .row-close {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 28px;
        flex: 0 0 28px;
        background: transparent;
        border: none;
        color: var(--ui-text-muted);
        cursor: pointer;
        opacity: 0;
        pointer-events: none;
        transition:
            opacity var(--ui-motion-fast),
            color var(--ui-motion-fast);
        padding: 0;
        border-radius: 4px;
    }

    .workspace-row:hover .row-close {
        opacity: 1;
        pointer-events: auto;
    }

    .row-close:hover {
        color: var(--ui-danger);
        background: color-mix(in srgb, var(--ui-danger) 10%, transparent);
    }
</style>
