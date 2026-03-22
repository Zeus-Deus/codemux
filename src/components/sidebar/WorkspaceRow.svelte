<script lang="ts">
    import type { WorkspaceSnapshot } from '../../stores/types';

    let {
        workspace,
        isActive,
        onActivate,
        onClose,
        onMarkRead,
        onOpenInEditor
    }: {
        workspace: WorkspaceSnapshot;
        isActive: boolean;
        onActivate: () => void;
        onClose: () => void;
        onMarkRead: () => void;
        onOpenInEditor?: () => void;
    } = $props();

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

    <div class="row-body" title={workspace.cwd}>
        <div class="row-top">
            <span
                class="row-dot"
                class:dot-active={isActive && workspace.notification_count === 0}
                class:dot-attention={workspace.notification_count > 0}
            ></span>
            <span class="row-name">{workspace.title}</span>
            {#if workspace.git_ahead > 0}
                <span class="git-badge git-ahead">↑{workspace.git_ahead}</span>
            {/if}
            {#if workspace.git_behind > 0}
                <span class="git-badge git-behind">↓{workspace.git_behind}</span>
            {/if}
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

        {#if workspace.git_branch}
            <div class="row-meta">
                <svg class="branch-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M6 3v12M18 9v12M6 3C6 3 6 9 12 9s6-6 6-6M6 15c0 0 0 6 6 6s6-6 6-6" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>
                </svg>
                <span class="meta-branch">{workspace.git_branch}</span>
                {#if workspace.git_additions > 0 || workspace.git_deletions > 0}
                    <span class="meta-sep">·</span>
                    {#if workspace.git_additions > 0}
                        <span class="diff-stat diff-add">+{workspace.git_additions}</span>
                    {/if}
                    {#if workspace.git_deletions > 0}
                        <span class="diff-stat diff-del">-{workspace.git_deletions}</span>
                    {/if}
                {/if}
            </div>
        {/if}
    </div>

    {#if onOpenInEditor}
        <button
            class="row-action row-editor"
            type="button"
            aria-label="Open in editor"
            title="Open in editor"
            onclick={(e) => { e.stopPropagation(); onOpenInEditor(); }}
        >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6M15 3h6v6M10 14L21 3" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
        </button>
    {/if}

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
        border-radius: var(--ui-radius-md);
        cursor: pointer;
        overflow: hidden;
        transition:
            background var(--ui-motion-fast),
            box-shadow var(--ui-motion-fast);
        outline: none;
        min-height: 38px;
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
        border-radius: var(--ui-radius-sm) 0 0 var(--ui-radius-sm);
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
        padding: 8px 6px 8px 10px;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .row-top {
        display: flex;
        align-items: center;
        gap: 7px;
        min-width: 0;
    }

    .git-badge {
        font-family: var(--ui-font-mono);
        font-size: 0.65rem;
        font-weight: 600;
        flex-shrink: 0;
    }

    .git-ahead { color: var(--ui-success); }
    .git-behind { color: var(--ui-attention); }

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
        border-radius: var(--ui-radius-lg);
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

    .branch-icon {
        flex-shrink: 0;
        color: var(--ui-text-muted);
    }

    .meta-branch {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .meta-sep {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        flex-shrink: 0;
    }

    .diff-stat {
        font-size: 0.7rem;
        font-weight: 600;
        flex-shrink: 0;
    }

    .diff-add { color: var(--ui-success); }
    .diff-del { color: var(--ui-danger); }

    .row-action {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 24px;
        flex: 0 0 24px;
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
        border-radius: var(--ui-radius-sm);
    }

    .workspace-row:hover .row-action {
        opacity: 1;
        pointer-events: auto;
    }

    .row-editor:hover {
        color: var(--ui-accent);
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
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
        border-radius: var(--ui-radius-sm);
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
