<script lang="ts">
    import { onMount } from 'svelte';
    import { ghStatus, refreshGhStatus, checkGithubRepo, getBranchPr, createPr, mergePr, getPrChecks } from '../../stores/github';
    import { listBranches } from '../../stores/git';
    import CustomSelect from '../ui/CustomSelect.svelte';
    import type { PullRequestInfo, CheckInfo } from '../../stores/types';

    let { workspaceCwd, onClose }: { workspaceCwd: string; onClose: () => void } = $props();

    let isGithubRepo = $state(true);
    let pr = $state<PullRequestInfo | null>(null);
    let checks = $state<CheckInfo[]>([]);
    let isLoading = $state(false);
    let error = $state<string | null>(null);

    // Create form state
    let title = $state('');
    let body = $state('');
    let baseBranch = $state('main');
    let isDraft = $state(false);
    let branches = $state<string[]>([]);

    // Merge state
    let mergeMethod = $state('squash');
    let mergeConfirm = $state(false);
    let isMerging = $state(false);

    let refreshTimer: ReturnType<typeof setInterval> | null = null;

    const allChecksPassing = $derived(checks.length > 0 && checks.every(c => c.status === 'pass'));
    const hasFailedChecks = $derived(checks.some(c => c.status === 'fail'));
    const hasPendingChecks = $derived(checks.some(c => c.status === 'pending'));

    async function init() {
        try {
            if ($ghStatus.status !== 'Authenticated') return;
            isGithubRepo = await checkGithubRepo(workspaceCwd);
            if (!isGithubRepo) return;
            await refresh();
            branches = await listBranches(workspaceCwd, false);
        } catch (e) {
            console.error('PR panel init:', e);
        }
    }

    async function refresh() {
        try {
            pr = await getBranchPr(workspaceCwd);
            if (pr) {
                checks = await getPrChecks(workspaceCwd);
            } else {
                checks = [];
            }
            error = null;
        } catch (e) {
            console.error('PR refresh:', e);
        }
    }

    async function handleCreate() {
        if (!title.trim()) return;
        isLoading = true;
        error = null;
        try {
            pr = await createPr(workspaceCwd, title.trim(), body.trim(), baseBranch || null, isDraft);
            title = '';
            body = '';
            checks = await getPrChecks(workspaceCwd);
        } catch (e: unknown) {
            error = e instanceof Error ? e.message : String(e);
        }
        isLoading = false;
    }

    async function handleMerge() {
        if (!pr || !mergeConfirm) {
            mergeConfirm = true;
            return;
        }
        isMerging = true;
        error = null;
        try {
            await mergePr(workspaceCwd, pr.number, mergeMethod);
            mergeConfirm = false;
            await refresh();
        } catch (e: unknown) {
            error = e instanceof Error ? e.message : String(e);
        }
        isMerging = false;
    }

    function branchToTitle(branch: string | null): string {
        if (!branch) return '';
        return branch
            .replace(/^(feature|fix|chore|docs|refactor|test)[/-]/, '')
            .replace(/[-_]/g, ' ')
            .replace(/^\w/, c => c.toUpperCase());
    }

    function prefillFromBranch() {
        if (!title && pr === null) {
            // Try to extract a title from the branch name via git
            getBranchPr(workspaceCwd).catch(() => null); // just to check
        }
    }

    function checkIcon(status: string): string {
        switch (status) {
            case 'pass': return 'check-pass';
            case 'fail': return 'check-fail';
            default: return 'check-pending';
        }
    }

    onMount(() => {
        void init();
        refreshTimer = setInterval(() => {
            if (pr) void refresh();
        }, 10_000);
        return () => {
            if (refreshTimer) clearInterval(refreshTimer);
        };
    });

    // Pre-fill title from branch name when in create mode
    $effect(() => {
        if (!pr && !title && branches.length > 0) {
            // Get current branch from the list context — gh pr view failed so no branch info there
            // We'll just leave a helpful placeholder
        }
    });
</script>

<div class="pr-panel">
    <div class="panel-header">
        <span class="panel-title">Pull Request</span>
        {#if pr}
            <span class="pr-number-badge">#{pr.number}</span>
        {/if}
        <div class="header-spacer"></div>
        {#if $ghStatus.status === 'Authenticated' && isGithubRepo}
            <button class="header-btn" onclick={() => void refresh()} title="Refresh" aria-label="Refresh">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M21 2v6h-6M3 22v-6h6" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                    <path d="M3 12a9 9 0 0115.36-6.36L21 8M21 12a9 9 0 01-15.36 6.36L3 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
        {/if}
        <button class="header-btn close-btn" onclick={onClose} title="Close panel" aria-label="Close PR panel">
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
            </svg>
        </button>
    </div>

    {#if $ghStatus.status === 'NotInstalled'}
        <div class="panel-empty">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12 9v4M12 17h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            <span>GitHub CLI (gh) is required for PR features.</span>
            <a class="install-link" href="https://cli.github.com" target="_blank" rel="noopener">Install gh CLI</a>
            <button class="recheck-btn" onclick={() => void refreshGhStatus().then(() => void init())}>Check again</button>
        </div>
    {:else if $ghStatus.status === 'NotAuthenticated'}
        <div class="panel-empty">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12 9v4M12 17h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
            <span>Not logged in to GitHub.</span>
            <code class="auth-command">gh auth login</code>
            <button class="recheck-btn" onclick={() => void refreshGhStatus().then(() => void init())}>Check again</button>
        </div>
    {:else if !isGithubRepo}
        <div class="panel-empty">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.87a3.37 3.37 0 00-.94-2.61c3.14-.35 6.44-1.54 6.44-7A5.44 5.44 0 0020 4.77 5.07 5.07 0 0019.91 1S18.73.65 16 2.48a13.38 13.38 0 00-7 0C6.27.65 5.09 1 5.09 1A5.07 5.07 0 005 4.77a5.44 5.44 0 00-1.5 3.78c0 5.42 3.3 6.61 6.44 7A3.37 3.37 0 009 18.13V22" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            <span>This repository is not hosted on GitHub.</span>
        </div>
    {:else if pr}
        <!-- PR View Mode -->
        <div class="pr-content">
            <div class="pr-header-section">
                <div class="pr-title-row">
                    <span class="pr-state-badge" class:open={pr.state === 'OPEN'} class:merged={pr.state === 'MERGED'} class:closed={pr.state === 'CLOSED'}>
                        {pr.is_draft ? 'Draft' : pr.state === 'OPEN' ? 'Open' : pr.state === 'MERGED' ? 'Merged' : 'Closed'}
                    </span>
                    <span class="pr-title-text">{pr.title}</span>
                </div>

                <div class="pr-meta">
                    {#if pr.base_branch && pr.head_branch}
                        <span class="pr-branches">
                            <span class="branch-name">{pr.base_branch}</span>
                            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                <path d="M7.5 5H2.5M2.5 5l2-2M2.5 5l2 2" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round"/>
                            </svg>
                            <span class="branch-name">{pr.head_branch}</span>
                        </span>
                    {/if}
                    {#if pr.additions != null || pr.deletions != null}
                        <span class="pr-diff-stat">
                            {#if pr.additions != null}<span class="diff-add">+{pr.additions}</span>{/if}
                            {#if pr.deletions != null}<span class="diff-del">-{pr.deletions}</span>{/if}
                        </span>
                    {/if}
                </div>

                {#if pr.review_decision}
                    <div class="pr-review" class:approved={pr.review_decision === 'APPROVED'} class:changes-requested={pr.review_decision === 'CHANGES_REQUESTED'} class:pending-review={pr.review_decision === 'REVIEW_REQUIRED'}>
                        {#if pr.review_decision === 'APPROVED'}
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>
                            Approved
                        {:else if pr.review_decision === 'CHANGES_REQUESTED'}
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M12 9v4M12 17h.01" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>
                            Changes requested
                        {:else}
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2"/></svg>
                            Review pending
                        {/if}
                    </div>
                {/if}
            </div>

            <!-- Checks -->
            {#if checks.length > 0}
                <div class="checks-section">
                    <div class="section-label">Checks</div>
                    <div class="checks-list">
                        {#each checks as check}
                            <div class="check-row">
                                <span class="check-icon {checkIcon(check.status)}">
                                    {#if check.status === 'pass'}
                                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M20 6L9 17l-5-5" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                                    {:else if check.status === 'fail'}
                                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"><path d="M18 6L6 18M6 6l12 12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"/></svg>
                                    {:else}
                                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none"><circle cx="12" cy="12" r="4" fill="currentColor"/></svg>
                                    {/if}
                                </span>
                                <span class="check-name">{check.name}</span>
                            </div>
                        {/each}
                    </div>
                </div>
            {/if}

            <!-- Merge controls -->
            {#if pr.state === 'OPEN'}
                <div class="merge-section">
                    <div class="section-label">Merge</div>
                    <div class="merge-controls">
                        <CustomSelect
                            compact
                            options={[
                                { value: 'squash', label: 'Squash and merge' },
                                { value: 'merge', label: 'Create merge commit' },
                                { value: 'rebase', label: 'Rebase and merge' },
                            ]}
                            bind:value={mergeMethod}
                        />
                        <button
                            class="merge-btn"
                            class:safe={allChecksPassing}
                            class:pending={hasPendingChecks}
                            class:danger={hasFailedChecks}
                            class:confirm={mergeConfirm}
                            disabled={isMerging || pr.mergeable === 'CONFLICTING'}
                            onclick={handleMerge}
                        >
                            {#if isMerging}
                                Merging...
                            {:else if mergeConfirm}
                                Confirm merge
                            {:else if pr.mergeable === 'CONFLICTING'}
                                Conflicts
                            {:else}
                                Merge
                            {/if}
                        </button>
                        {#if mergeConfirm}
                            <button class="merge-cancel" onclick={() => mergeConfirm = false}>Cancel</button>
                        {/if}
                    </div>
                </div>
            {/if}

            <!-- View on GitHub -->
            {#if pr.url}
                <div class="pr-actions">
                    <a class="github-link" href={pr.url} target="_blank" rel="noopener">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                            <path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6M15 3h6v6M10 14L21 3" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                        View on GitHub
                    </a>
                </div>
            {/if}

            {#if error}
                <div class="pr-error">{error}</div>
            {/if}
        </div>
    {:else}
        <!-- Create PR Mode -->
        <div class="pr-content">
            <div class="create-form">
                <div class="form-group">
                    <label class="form-label" for="pr-title">Title</label>
                    <input
                        id="pr-title"
                        class="form-input"
                        type="text"
                        bind:value={title}
                        placeholder={branchToTitle(null) || 'Pull request title'}
                        onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void handleCreate(); }}}
                    />
                </div>

                <div class="form-group">
                    <label class="form-label" for="pr-body">Description</label>
                    <textarea
                        id="pr-body"
                        class="form-textarea"
                        bind:value={body}
                        placeholder="Optional description (markdown)"
                        rows="4"
                    ></textarea>
                </div>

                <div class="form-row">
                    <div class="form-group form-group-inline">
                        <label class="form-label" for="pr-base">Base</label>
                        <CustomSelect
                            compact
                            id="pr-base"
                            options={[
                                ...branches.map(b => ({ value: b, label: b })),
                                ...(!branches.includes(baseBranch) ? [{ value: baseBranch, label: baseBranch }] : []),
                            ]}
                            bind:value={baseBranch}
                        />
                    </div>

                    <label class="form-checkbox">
                        <input type="checkbox" bind:checked={isDraft} />
                        <span>Draft</span>
                    </label>
                </div>

                <button
                    class="create-btn"
                    disabled={isLoading || !title.trim()}
                    onclick={handleCreate}
                >
                    {isLoading ? 'Creating...' : 'Create Pull Request'}
                </button>

                {#if error}
                    <div class="pr-error">{error}</div>
                {/if}
            </div>
        </div>
    {/if}
</div>

<style>
    .pr-panel {
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

    .pr-number-badge {
        font-family: var(--ui-font-mono);
        font-size: 0.68rem;
        font-weight: 600;
        padding: 1px 6px;
        border-radius: var(--ui-radius-md);
        background: color-mix(in srgb, var(--ui-accent) 15%, transparent);
        color: var(--ui-accent);
    }

    .header-spacer { flex: 1; }

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

    .panel-empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 8px;
        padding: 32px 16px;
        color: var(--ui-text-muted);
        font-size: 0.78rem;
        text-align: center;
    }

    .install-link {
        color: var(--ui-accent);
        text-decoration: none;
        font-size: 0.75rem;
    }

    .install-link:hover {
        text-decoration: underline;
    }

    .auth-command {
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        padding: 4px 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        user-select: all;
    }

    .recheck-btn {
        padding: 4px 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
        font-size: 0.72rem;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
    }

    .recheck-btn:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    /* PR content */
    .pr-content {
        flex: 1;
        overflow-y: auto;
        overflow-x: hidden;
        min-height: 0;
    }

    /* PR header section */
    .pr-header-section {
        padding: 12px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .pr-title-row {
        display: flex;
        align-items: flex-start;
        gap: 8px;
        margin-bottom: 8px;
    }

    .pr-state-badge {
        font-size: 0.68rem;
        font-weight: 600;
        padding: 2px 8px;
        border-radius: var(--ui-radius-md);
        flex-shrink: 0;
        white-space: nowrap;
    }

    .pr-state-badge.open {
        color: #3fb950;
        background: color-mix(in srgb, #3fb950 15%, transparent);
    }

    .pr-state-badge.merged {
        color: #a371f7;
        background: color-mix(in srgb, #a371f7 15%, transparent);
    }

    .pr-state-badge.closed {
        color: var(--ui-danger);
        background: color-mix(in srgb, var(--ui-danger) 15%, transparent);
    }

    .pr-title-text {
        font-size: 0.82rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        line-height: 1.4;
    }

    .pr-meta {
        display: flex;
        align-items: center;
        gap: 10px;
        font-size: 0.72rem;
        color: var(--ui-text-muted);
    }

    .pr-branches {
        display: flex;
        align-items: center;
        gap: 4px;
    }

    .branch-name {
        font-family: var(--ui-font-mono);
        font-size: 0.7rem;
        padding: 1px 5px;
        background: var(--ui-layer-2);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
    }

    .pr-diff-stat {
        display: flex;
        gap: 6px;
        font-family: var(--ui-font-mono);
        font-size: 0.7rem;
    }

    .diff-add { color: var(--ui-success); }
    .diff-del { color: var(--ui-danger); }

    /* Review status */
    .pr-review {
        display: flex;
        align-items: center;
        gap: 6px;
        margin-top: 8px;
        font-size: 0.72rem;
        padding: 4px 8px;
        border-radius: var(--ui-radius-sm);
    }

    .pr-review.approved {
        color: #3fb950;
        background: color-mix(in srgb, #3fb950 10%, transparent);
    }

    .pr-review.changes-requested {
        color: var(--ui-attention);
        background: color-mix(in srgb, var(--ui-attention) 10%, transparent);
    }

    .pr-review.pending-review {
        color: var(--ui-text-muted);
        background: var(--ui-layer-2);
    }

    /* Checks */
    .checks-section {
        padding: 10px 12px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .section-label {
        font-size: 0.7rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
        margin-bottom: 6px;
    }

    .checks-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .check-row {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 3px 4px;
        border-radius: var(--ui-radius-sm);
    }

    .check-icon {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 14px;
        height: 14px;
        flex-shrink: 0;
    }

    .check-icon.check-pass { color: #3fb950; }
    .check-icon.check-fail { color: var(--ui-danger); }
    .check-icon.check-pending { color: var(--ui-attention); }

    .check-name {
        font-size: 0.72rem;
        color: var(--ui-text-secondary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    /* Merge section */
    .merge-section {
        padding: 10px 12px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .merge-controls {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .merge-method {
        width: 100%;
        height: 32px;
        padding: 0 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-size: 0.75rem;
        outline: none;
        cursor: pointer;
        box-shadow: var(--ui-shadow-xs);
    }

    .merge-btn {
        width: 100%;
        height: 32px;
        padding: 0 12px;
        border: none;
        border-radius: var(--ui-radius-sm);
        font-size: 0.78rem;
        font-weight: 600;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
        background: var(--ui-accent);
        color: var(--ui-layer-0);
    }

    .merge-btn.safe { background: var(--ui-success); }
    .merge-btn.pending { background: var(--ui-attention); color: var(--ui-layer-0); }
    .merge-btn.danger { background: var(--ui-danger); }
    .merge-btn.confirm { background: var(--ui-attention); }

    .merge-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .merge-btn:not(:disabled):hover {
        opacity: 0.9;
    }

    .merge-cancel {
        width: 100%;
        height: 28px;
        padding: 0 12px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
        font-size: 0.75rem;
        cursor: pointer;
    }

    .merge-cancel:hover {
        background: var(--ui-layer-3);
    }

    /* Actions */
    .pr-actions {
        padding: 10px 12px;
    }

    .github-link {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        width: 100%;
        height: 28px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-secondary);
        font-size: 0.75rem;
        text-decoration: none;
        cursor: pointer;
        transition: all var(--ui-motion-fast);
    }

    .github-link:hover {
        background: var(--ui-layer-3);
        color: var(--ui-text-primary);
    }

    .pr-error {
        margin: 8px 12px;
        padding: 6px 10px;
        background: color-mix(in srgb, var(--ui-danger) 10%, transparent);
        border: 1px solid color-mix(in srgb, var(--ui-danger) 25%, transparent);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-danger);
        font-size: 0.72rem;
        word-break: break-word;
    }

    /* Create form */
    .create-form {
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .form-group {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .form-group-inline {
        flex: 1;
        min-width: 0;
    }

    .form-label {
        font-size: 0.7rem;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.04em;
        color: var(--ui-text-muted);
    }

    .form-input {
        width: 100%;
        height: 30px;
        padding: 0 8px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-size: 0.78rem;
        outline: none;
        box-sizing: border-box;
        transition: border-color var(--ui-motion-fast);
    }

    .form-input:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
    }

    .form-input::placeholder {
        color: var(--ui-text-muted);
    }

    .form-textarea {
        width: 100%;
        padding: 6px 8px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-size: 0.75rem;
        font-family: var(--ui-font-mono);
        outline: none;
        box-sizing: border-box;
        resize: vertical;
        min-height: 60px;
        transition: border-color var(--ui-motion-fast);
    }

    .form-textarea:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
    }

    .form-textarea::placeholder {
        color: var(--ui-text-muted);
    }

    .form-row {
        display: flex;
        align-items: flex-end;
        gap: 10px;
    }

    .form-select {
        width: 100%;
        height: 32px;
        padding: 0 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-size: 0.75rem;
        outline: none;
        cursor: pointer;
        box-shadow: var(--ui-shadow-xs);
    }

    .form-checkbox {
        display: flex;
        align-items: center;
        gap: 5px;
        font-size: 0.75rem;
        color: var(--ui-text-secondary);
        cursor: pointer;
        white-space: nowrap;
        padding-bottom: 4px;
    }

    .form-checkbox input {
        accent-color: var(--ui-accent);
    }

    .create-btn {
        width: 100%;
        height: 32px;
        padding: 0 12px;
        background: var(--ui-accent);
        border: none;
        border-radius: var(--ui-radius-sm);
        color: var(--ui-layer-0);
        font-size: 0.78rem;
        font-weight: 600;
        cursor: pointer;
        transition: opacity var(--ui-motion-fast);
    }

    .create-btn:disabled {
        opacity: 0.4;
        cursor: default;
    }

    .create-btn:not(:disabled):hover {
        opacity: 0.9;
    }
</style>
