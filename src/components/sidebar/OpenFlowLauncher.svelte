<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import {
        openflowRuntime,
        createOpenFlowRun,
        advanceOpenFlowRunPhase,
        retryOpenFlowRun,
        runOpenFlowAutonomousLoop,
        applyOpenFlowReviewResult,
        stopOpenFlowRun
    } from '../../stores/appState';
    import type { OpenFlowRunRecord } from '../../stores/appState';

    let expanded = $state(false);
    let composing = $state(false);
    let titleDraft = $state('');
    let goalDraft = $state('');

    const dispatch = createEventDispatcher<{ newrun: void }>();

    const runs = $derived($openflowRuntime?.active_runs ?? []);
    const runCount = $derived(runs.length);

    function runTone(status: OpenFlowRunRecord['status']) {
        if (status === 'completed') return 'ready';
        if (status === 'failed' || status === 'cancelled') return 'danger';
        if (status === 'awaiting_approval') return 'attention';
        return 'busy';
    }

    function latestMessage(run: OpenFlowRunRecord) {
        return run.timeline[run.timeline.length - 1]?.message ?? run.goal;
    }

    function needsReview(run: OpenFlowRunRecord) {
        return run.current_phase === 'review' || run.status === 'awaiting_approval';
    }

    async function handleStart() {
        if (!titleDraft.trim() || !goalDraft.trim()) return;
        try {
            await createOpenFlowRun({ title: titleDraft.trim(), goal: goalDraft.trim() });
            titleDraft = '';
            goalDraft = '';
            composing = false;
        } catch (error) {
            console.error('Failed to start run:', error);
        }
    }

    async function handleAdvance(runId: string) {
        try { await advanceOpenFlowRunPhase(runId); } catch (e) { console.error(e); }
    }

    async function handleRetry(runId: string) {
        try { await retryOpenFlowRun(runId); } catch (e) { console.error(e); }
    }

    async function handleLoop(runId: string) {
        try { await runOpenFlowAutonomousLoop(runId); } catch (e) { console.error(e); }
    }

    async function handleApprove(runId: string) {
        try { await applyOpenFlowReviewResult(runId, 95, true, null); } catch (e) { console.error(e); }
    }

    async function handleReject(runId: string) {
        try { await applyOpenFlowReviewResult(runId, 58, false, 'Reviewer requested fixes'); } catch (e) { console.error(e); }
    }

    async function handlePause(runId: string) {
        try { await stopOpenFlowRun(runId, 'awaiting_approval', 'Paused by user'); } catch (e) { console.error(e); }
    }

    async function handleCancel(runId: string) {
        try { await stopOpenFlowRun(runId, 'cancelled', 'Cancelled by user'); } catch (e) { console.error(e); }
    }
</script>

<div class="section">
    <div class="section-header">
        <button
            class="header-toggle"
            type="button"
            onclick={() => (expanded = !expanded)}
        >
            <span class="section-label">OpenFlow</span>
            {#if runCount > 0}
                <span class="run-badge">{runCount}</span>
            {/if}
            <span class="spacer"></span>
            <span class="chevron" class:open={expanded}>›</span>
        </button>

        <button
            class="compose-toggle"
            type="button"
            title="New run"
            onclick={() => dispatch('newrun')}
            aria-label="New OpenFlow run"
        >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                <path d="M6 1v10M1 6h10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
        </button>
    </div>

    {#if expanded}
        <div class="section-body">
            {#if composing}
                <div class="compose-form">
                    <input
                        class="compose-input"
                        bind:value={titleDraft}
                        placeholder="Run title"
                        onkeydown={(e) => { if (e.key === 'Escape') composing = false; }}
                    />
                    <textarea
                        class="compose-goal"
                        bind:value={goalDraft}
                        rows="3"
                        placeholder="Describe the goal…"
                        onkeydown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); composing = false; } }}
                    ></textarea>
                    <div class="compose-actions">
                        <button
                            class="start-btn"
                            type="button"
                            onclick={handleStart}
                            disabled={!titleDraft.trim() || !goalDraft.trim()}
                        >
                            Start run
                        </button>
                        <button
                            class="cancel-btn"
                            type="button"
                            onclick={() => (composing = false)}
                        >
                            Cancel
                        </button>
                    </div>
                </div>
            {/if}

            {#if runs.length > 0}
                <div class="run-list">
                    {#each runs as run (run.run_id)}
                        <div class="run-row">
                            <div class="run-main">
                                <span class="run-dot {runTone(run.status)}"></span>
                                <div class="run-info">
                                    <span class="run-title">{run.title}</span>
                                    <span class="run-latest">{latestMessage(run)}</span>
                                </div>
                                <span class="run-phase-badge {runTone(run.status)}">{run.current_phase}</span>
                            </div>
                            <div class="run-actions">
                                {#if run.status !== 'completed' && run.status !== 'cancelled' && run.status !== 'failed'}
                                    <button class="run-btn" type="button" onclick={() => handleLoop(run.run_id)} title="Run loop">Loop</button>
                                    <button class="run-btn" type="button" onclick={() => handleAdvance(run.run_id)} title="Advance phase">Next</button>
                                    <button class="run-btn" type="button" onclick={() => handlePause(run.run_id)} title="Pause">Pause</button>
                                    <button class="run-btn danger" type="button" onclick={() => handleCancel(run.run_id)} title="Cancel">Cancel</button>
                                {:else}
                                    <button class="run-btn" type="button" onclick={() => handleRetry(run.run_id)} title="Retry">Retry</button>
                                {/if}
                                {#if needsReview(run)}
                                    <button class="run-btn accent" type="button" onclick={() => handleApprove(run.run_id)}>Approve</button>
                                    <button class="run-btn" type="button" onclick={() => handleReject(run.run_id)}>Reject</button>
                                {/if}
                            </div>
                        </div>
                    {/each}
                </div>
            {:else if !composing}
                <p class="empty-hint">No active runs. Press <kbd>+</kbd> to start one.</p>
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
        gap: 4px;
    }

    .header-toggle {
        display: flex;
        align-items: center;
        gap: 7px;
        flex: 1;
        min-width: 0;
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
        border-radius: 6px 0 0 6px;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
        text-align: left;
    }

    .header-toggle:hover {
        background: color-mix(in srgb, var(--ui-accent) 6%, transparent);
        color: var(--ui-text-primary);
    }

    .section-label {
        flex-shrink: 0;
    }

    .run-badge {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-width: 18px;
        height: 18px;
        padding: 0 5px;
        border-radius: 9px;
        background: color-mix(in srgb, var(--ui-accent) 20%, transparent);
        color: var(--ui-accent);
        font-size: 0.66rem;
        font-weight: 700;
        flex-shrink: 0;
        border: 1px solid color-mix(in srgb, var(--ui-accent) 30%, transparent);
    }

    .spacer { flex: 1; }

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

    .compose-toggle {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 28px;
        height: 32px;
        flex: 0 0 28px;
        background: transparent;
        border: none;
        border-radius: 6px;
        color: var(--ui-text-muted);
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
        margin-right: 6px;
    }

    .compose-toggle:hover {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        color: var(--ui-accent);
    }

    .section-body {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 2px 8px 8px;
    }

    .compose-form {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px;
        background: var(--ui-layer-2);
        border-radius: 6px;
        border: 1px solid var(--ui-border-soft);
    }

    .compose-input,
    .compose-goal {
        width: 100%;
        box-sizing: border-box;
        background: var(--ui-layer-1);
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.8rem;
        padding: 7px 9px;
        transition: border-color var(--ui-motion-fast);
        resize: none;
        outline: none;
    }

    .compose-input:focus,
    .compose-goal:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 40%, transparent);
    }

    .compose-goal {
        resize: vertical;
        min-height: 56px;
    }

    .compose-actions {
        display: flex;
        gap: 6px;
    }

    .start-btn {
        flex: 1;
        padding: 7px 12px;
        background: color-mix(in srgb, var(--ui-accent) 16%, var(--ui-layer-2) 84%);
        border: 1px solid color-mix(in srgb, var(--ui-accent) 30%, transparent);
        border-radius: 5px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.78rem;
        font-weight: 600;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
    }

    .start-btn:hover:not(:disabled) {
        background: color-mix(in srgb, var(--ui-accent) 24%, var(--ui-layer-2) 76%);
    }

    .start-btn:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }

    .cancel-btn {
        padding: 7px 10px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.78rem;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .cancel-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .run-list {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .run-row {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 8px 10px;
        background: var(--ui-layer-2);
        border-radius: 6px;
        border: 1px solid var(--ui-border-soft);
    }

    .run-main {
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 0;
    }

    .run-dot {
        width: 7px;
        height: 7px;
        border-radius: 50%;
        flex: 0 0 7px;
        background: var(--ui-text-muted);
    }

    .run-dot.busy {
        background: var(--ui-accent);
        animation: pulse-dot 1.4s ease-in-out infinite;
    }

    .run-dot.ready { background: var(--ui-success); }
    .run-dot.danger { background: var(--ui-danger); }
    .run-dot.attention { background: var(--ui-attention); }

    @keyframes pulse-dot {
        0%, 100% { opacity: 1; }
        50% { opacity: 0.4; }
    }

    .run-info {
        flex: 1;
        min-width: 0;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .run-title {
        font-size: 0.8rem;
        font-weight: 600;
        color: var(--ui-text-primary);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .run-latest {
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }

    .run-phase-badge {
        font-size: 0.68rem;
        font-weight: 600;
        padding: 2px 7px;
        border-radius: 4px;
        background: var(--ui-layer-1);
        border: 1px solid var(--ui-border-soft);
        color: var(--ui-text-muted);
        white-space: nowrap;
        flex-shrink: 0;
    }

    .run-phase-badge.busy {
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 25%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
    }

    .run-phase-badge.ready {
        color: var(--ui-success);
        border-color: color-mix(in srgb, var(--ui-success) 25%, transparent);
        background: color-mix(in srgb, var(--ui-success) 10%, transparent);
    }

    .run-phase-badge.attention {
        color: var(--ui-attention);
        border-color: color-mix(in srgb, var(--ui-attention) 25%, transparent);
        background: color-mix(in srgb, var(--ui-attention) 10%, transparent);
    }

    .run-phase-badge.danger {
        color: var(--ui-danger);
        border-color: color-mix(in srgb, var(--ui-danger) 25%, transparent);
        background: color-mix(in srgb, var(--ui-danger) 10%, transparent);
    }

    .run-actions {
        display: flex;
        flex-wrap: wrap;
        gap: 5px;
    }

    .run-btn {
        padding: 4px 8px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 4px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.72rem;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
    }

    .run-btn:hover {
        background: var(--ui-layer-1);
        color: var(--ui-text-secondary);
    }

    .run-btn.accent {
        color: var(--ui-accent);
        border-color: color-mix(in srgb, var(--ui-accent) 25%, transparent);
        background: color-mix(in srgb, var(--ui-accent) 8%, transparent);
    }

    .run-btn.accent:hover {
        background: color-mix(in srgb, var(--ui-accent) 16%, transparent);
    }

    .run-btn.danger:hover {
        color: var(--ui-danger);
        border-color: color-mix(in srgb, var(--ui-danger) 25%, transparent);
        background: color-mix(in srgb, var(--ui-danger) 8%, transparent);
    }

    .empty-hint {
        margin: 0;
        font-size: 0.76rem;
        color: var(--ui-text-muted);
        padding: 4px 8px;
        line-height: 1.4;
    }

    kbd {
        font-family: inherit;
        font-size: 0.76em;
        padding: 1px 4px;
        border: 1px solid var(--ui-border-soft);
        border-radius: 3px;
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }
</style>
