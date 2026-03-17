<script lang="ts">
    import {
        projectMemory,
        updateProjectMemory,
        addProjectMemoryEntry,
        generateProjectHandoff,
    } from '../../stores/memory';

    let expanded = $state(false);
    let activeTab = $state<'brief' | 'goal' | 'notes'>('brief');

    let briefDraft = $state('');
    let goalDraft = $state('');
    let focusDraft = $state('');
    let constraintsDraft = $state('');
    let entryDraft = $state('');
    let handoffPrompt = $state('');

    $effect(() => {
        const mem = $projectMemory;
        if (!mem) return;
        briefDraft = mem.project_brief ?? '';
        goalDraft = mem.current_goal ?? '';
        focusDraft = mem.current_focus ?? '';
        constraintsDraft = mem.constraints.join('\n');
    });

    async function handleSave() {
        try {
            await updateProjectMemory({
                project_brief: briefDraft,
                current_goal: goalDraft,
                current_focus: focusDraft,
                constraints: constraintsDraft.split('\n').map((s) => s.trim()).filter(Boolean)
            });
        } catch (error) {
            console.error('Failed to save memory:', error);
        }
    }

    async function handleAddEntry(kind: 'pinned_context' | 'decision' | 'next_step' | 'session_summary') {
        if (!entryDraft.trim()) return;
        try {
            await addProjectMemoryEntry(kind, entryDraft.trim(), { toolName: 'codemux-ui' });
            entryDraft = '';
        } catch (error) {
            console.error('Failed to add memory entry:', error);
        }
    }

    async function handleHandoff() {
        try {
            const packet = await generateProjectHandoff();
            handoffPrompt = packet.suggested_prompt;
        } catch (error) {
            console.error('Failed to generate handoff:', error);
        }
    }

    function projectName() {
        return $projectMemory?.project_name ?? 'project';
    }
</script>

<div class="section">
    <button
        class="section-header"
        type="button"
        onclick={() => (expanded = !expanded)}
    >
        <span class="section-label">Memory</span>
        <span class="project-name">{projectName()}</span>
        <span class="spacer"></span>
        <span class="chevron" class:open={expanded}>›</span>
    </button>

    {#if expanded}
        <div class="section-body">
            <div class="tab-row">
                <button
                    class="tab-btn"
                    class:active={activeTab === 'brief'}
                    type="button"
                    onclick={() => (activeTab = 'brief')}
                >Brief</button>
                <button
                    class="tab-btn"
                    class:active={activeTab === 'goal'}
                    type="button"
                    onclick={() => (activeTab = 'goal')}
                >Goal</button>
                <button
                    class="tab-btn"
                    class:active={activeTab === 'notes'}
                    type="button"
                    onclick={() => (activeTab = 'notes')}
                >Notes</button>
            </div>

            {#if activeTab === 'brief'}
                <div class="tab-content">
                    <textarea
                        class="mem-textarea"
                        bind:value={briefDraft}
                        rows="4"
                        placeholder="What is this project trying to become?"
                    ></textarea>
                    <textarea
                        class="mem-textarea"
                        bind:value={constraintsDraft}
                        rows="3"
                        placeholder="Constraints (one per line)"
                    ></textarea>
                    <button class="save-btn" type="button" onclick={handleSave}>Save</button>
                </div>
            {:else if activeTab === 'goal'}
                <div class="tab-content">
                    <textarea
                        class="mem-textarea"
                        bind:value={goalDraft}
                        rows="3"
                        placeholder="What are we building right now?"
                    ></textarea>
                    <textarea
                        class="mem-textarea"
                        bind:value={focusDraft}
                        rows="3"
                        placeholder="What should the next session focus on?"
                    ></textarea>
                    <button class="save-btn" type="button" onclick={handleSave}>Save</button>
                </div>
            {:else}
                <div class="tab-content">
                    <textarea
                        class="mem-textarea"
                        bind:value={entryDraft}
                        rows="4"
                        placeholder="Pin context, record a decision, or capture a next step…"
                    ></textarea>
                    <div class="entry-actions">
                        <button class="entry-btn" type="button" onclick={() => handleAddEntry('pinned_context')}>Pin</button>
                        <button class="entry-btn" type="button" onclick={() => handleAddEntry('decision')}>Decision</button>
                        <button class="entry-btn" type="button" onclick={() => handleAddEntry('next_step')}>Next step</button>
                    </div>
                    <button class="save-btn" type="button" onclick={handleHandoff}>Generate handoff</button>
                    {#if handoffPrompt}
                        <textarea
                            class="mem-textarea handoff-output"
                            readonly
                            rows="8"
                            value={handoffPrompt}
                        ></textarea>
                    {/if}
                </div>
            {/if}

            {#if $projectMemory}
                <div class="memory-stats">
                    <span class="stat">{$projectMemory.pinned_context.length} pinned</span>
                    <span class="stat-sep">·</span>
                    <span class="stat">{$projectMemory.recent_decisions.length} decisions</span>
                    <span class="stat-sep">·</span>
                    <span class="stat">{$projectMemory.next_steps.length} next steps</span>
                </div>
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

    .project-name {
        font-size: 0.72rem;
        font-weight: 400;
        letter-spacing: 0;
        text-transform: none;
        color: var(--ui-text-muted);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        max-width: 80px;
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

    .section-body {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 2px 8px 10px;
    }

    .tab-row {
        display: flex;
        gap: 4px;
    }

    .tab-btn {
        flex: 1;
        padding: 5px 6px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.74rem;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
    }

    .tab-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .tab-btn.active {
        background: color-mix(in srgb, var(--ui-accent) 12%, transparent);
        border-color: color-mix(in srgb, var(--ui-accent) 28%, transparent);
        color: var(--ui-text-primary);
    }

    .tab-content {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }

    .mem-textarea {
        width: 100%;
        box-sizing: border-box;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.78rem;
        line-height: 1.45;
        padding: 7px 9px;
        resize: vertical;
        transition: border-color var(--ui-motion-fast);
        outline: none;
    }

    .mem-textarea:focus {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
    }

    .handoff-output {
        min-height: 160px;
        color: var(--ui-text-secondary);
        font-size: 0.74rem;
    }

    .save-btn {
        align-self: flex-start;
        padding: 6px 12px;
        background: color-mix(in srgb, var(--ui-accent) 14%, var(--ui-layer-2) 86%);
        border: 1px solid color-mix(in srgb, var(--ui-accent) 28%, transparent);
        border-radius: 5px;
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.76rem;
        font-weight: 600;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
    }

    .save-btn:hover {
        background: color-mix(in srgb, var(--ui-accent) 22%, var(--ui-layer-2) 78%);
    }

    .entry-actions {
        display: flex;
        gap: 5px;
        flex-wrap: wrap;
    }

    .entry-btn {
        padding: 5px 10px;
        background: transparent;
        border: 1px solid var(--ui-border-soft);
        border-radius: 5px;
        color: var(--ui-text-muted);
        font: inherit;
        font-size: 0.74rem;
        cursor: pointer;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .entry-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-secondary);
    }

    .memory-stats {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 2px 2px;
    }

    .stat {
        font-size: 0.7rem;
        color: var(--ui-text-muted);
    }

    .stat-sep {
        font-size: 0.7rem;
        color: var(--ui-text-muted);
        opacity: 0.5;
    }
</style>
