<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import { open } from '@tauri-apps/plugin-dialog';
    import {
        createWorkspaceWithPreset,
        type LayoutPreset,
        type WorkspaceTemplateKind
    } from '../../stores/appState';

    type LauncherStep = 'kind' | 'layout' | 'details';

    let {
        initialKind = 'codemux',
        initialLayout = 'single'
    }: {
        initialKind?: WorkspaceTemplateKind;
        initialLayout?: LayoutPreset;
    } = $props();

    const dispatch = createEventDispatcher<{ close: void }>();

    const kindOptions: Array<{
        kind: WorkspaceTemplateKind;
        title: string;
        summary: string;
        detail: string;
    }> = [
        {
            kind: 'codemux',
            title: 'Codemux workspace',
            summary: 'Start with shells in the current project',
            detail: 'Best for focused terminal work with optional browser panes later.'
        },
        {
            kind: 'folder',
            title: 'Open folder',
            summary: 'Use the current repo as a shell workspace',
            detail: 'Folder picking can be added later; for now this mirrors the current project root.'
        },
        {
            kind: 'openflow',
            title: 'OpenFlow run',
            summary: 'Create a shell workspace and attach a run goal',
            detail: 'Starts with the same pane layout, then creates a compact OpenFlow run on top.'
        }
    ];

    const layoutOptions: Array<{
        layout: LayoutPreset;
        title: string;
        subtitle: string;
        slots: number;
    }> = [
        { layout: 'single', title: '1 slot', subtitle: 'Single shell', slots: 1 },
        { layout: 'pair', title: '2 slots', subtitle: 'Side-by-side shells', slots: 2 },
        { layout: 'quad', title: '4 slots', subtitle: 'Balanced split grid', slots: 4 },
        { layout: 'six', title: '6 slots', subtitle: 'Two rows of three shells', slots: 6 },
        { layout: 'eight', title: '8 slots', subtitle: 'Two rows of four shells', slots: 8 },
        { layout: 'shell_browser', title: 'Shell + browser', subtitle: 'One shell with a browser companion', slots: 2 }
    ];

    let launcherEl = $state<HTMLDivElement | null>(null);
    let step = $state<LauncherStep>('kind');
    let selectedKind = $state<WorkspaceTemplateKind>('codemux');
    let selectedLayout = $state<LayoutPreset>('single');
    let openflowTitle = $state('');
    let openflowGoal = $state('');
    let selectedFolder = $state('');
    let creating = $state(false);

    const stepMeta = $derived.by(() => {
        if (step === 'kind') {
            return {
                index: 1,
                total: 3,
                label: 'Step 1',
                title: 'Choose workspace type',
                description: 'Start by choosing what kind of workspace you want to create.'
            };
        }

        if (step === 'layout') {
            return {
                index: 2,
                total: 3,
                label: 'Step 2',
                title: 'Choose layout preset',
                description: 'Pick the initial shell layout. You can always add more manually later.'
            };
        }

        return {
            index: 3,
            total: 3,
            label: 'Step 3',
            title: selectedKind === 'openflow' ? 'Add run details' : selectedKind === 'folder' ? 'Choose working folder' : 'Review setup',
            description: selectedKind === 'openflow'
                ? 'Add the run title and goal before creating the workspace.'
                : selectedKind === 'folder'
                    ? 'Pick the folder this workspace should open in.'
                    : 'Review the selection and create the workspace.'
        };
    });

    $effect(() => {
        selectedKind = initialKind;
        selectedLayout = initialLayout;
        step = 'kind';
        selectedFolder = '';
    });

    function goToLayout() {
        if (step === 'kind') {
            step = 'layout';
        }
    }

    function goToDetails() {
        if (step === 'layout') {
            step = 'details';
        }
    }

    function chooseKind(kind: WorkspaceTemplateKind) {
        selectedKind = kind;
        selectedFolder = '';
        goToLayout();
    }

    function chooseLayout(layout: LayoutPreset) {
        selectedLayout = layout;
        goToDetails();
    }

    function canCreate() {
        if (selectedKind === 'folder' && !selectedFolder.trim()) {
            return false;
        }

        if (selectedKind !== 'openflow') {
            return true;
        }

        return openflowTitle.trim().length > 0 && openflowGoal.trim().length > 0;
    }

    async function handleCreate() {
        if (!canCreate() || creating) {
            return;
        }

        creating = true;
        try {
            await createWorkspaceWithPreset({
                kind: selectedKind,
                layout: selectedLayout,
                cwd: selectedKind === 'folder' ? selectedFolder : undefined,
                openflowTitle,
                openflowGoal
            });
            dispatch('close');
        } catch (error) {
            console.error('Failed to create workspace preset:', error);
        } finally {
            creating = false;
        }
    }

    function slotPreview(layout: LayoutPreset) {
        if (layout === 'shell_browser') {
            return ['shell', 'browser'];
        }

        return Array.from({ length: layoutOptions.find((option) => option.layout === layout)?.slots ?? 1 }, () => 'shell');
    }

    async function chooseFolder() {
        const selection = await open({
            directory: true,
            multiple: false,
            title: 'Choose workspace folder'
        });

        if (typeof selection === 'string') {
            selectedFolder = selection;
        }
    }
</script>

<div class="launcher-backdrop" role="presentation" onclick={() => dispatch('close')}>
    <div
        bind:this={launcherEl}
        class="launcher-shell"
        role="dialog"
        aria-modal="true"
        aria-label="New workspace"
        tabindex="-1"
        onclick={(event) => event.stopPropagation()}
        onkeydown={(event) => {
            if (event.key === 'Escape') {
                dispatch('close');
            }
        }}
    >
        <header class="launcher-header">
            <div>
                <p class="eyebrow">New workspace</p>
                <h2>{stepMeta.title}</h2>
                <p class="subcopy">{stepMeta.description}</p>
            </div>
            <button class="close-button" type="button" onclick={() => dispatch('close')} aria-label="Close launcher">Close</button>
        </header>

        <div class="step-strip" aria-live="polite">
            <span class="step-chip">{stepMeta.label}</span>
            <div class="step-track" aria-hidden="true">
                {#each Array.from({ length: stepMeta.total }) as _, index}
                    <span class:active={index < stepMeta.index}></span>
                {/each}
            </div>
            <span class="step-count">{stepMeta.index} / {stepMeta.total}</span>
        </div>

        {#if step === 'kind'}
            <div class="card-grid kind-grid">
                {#each kindOptions as option}
                    <button class="option-card" type="button" onclick={() => chooseKind(option.kind)}>
                        <strong>{option.title}</strong>
                        <span>{option.summary}</span>
                        <small>{option.detail}</small>
                    </button>
                {/each}
            </div>
        {:else if step === 'layout'}
            <div class="card-grid layout-grid">
                {#each layoutOptions as option}
                    <button class="layout-card" type="button" onclick={() => chooseLayout(option.layout)}>
                        <div class={`layout-preview layout-${option.layout}`} class:browser-mix={option.layout === 'shell_browser'}>
                            {#each slotPreview(option.layout) as slot}
                                <span class:browser={slot === 'browser'}></span>
                            {/each}
                        </div>
                        <strong>{option.title}</strong>
                        <span>{option.subtitle}</span>
                    </button>
                {/each}
            </div>
        {:else}
            <div class="details-view">
                <div class="selection-summary">
                    <div>
                        <p class="eyebrow">Workspace type</p>
                        <strong>{kindOptions.find((option) => option.kind === selectedKind)?.title}</strong>
                    </div>
                    <div>
                        <p class="eyebrow">Layout preset</p>
                        <strong>{layoutOptions.find((option) => option.layout === selectedLayout)?.title}</strong>
                    </div>
                </div>

                {#if selectedKind === 'openflow'}
                    <div class="field-stack">
                        <label>
                            <span>Run title</span>
                            <input bind:value={openflowTitle} placeholder="Release polish" />
                        </label>
                        <label>
                            <span>Run goal</span>
                            <textarea bind:value={openflowGoal} rows="4" placeholder="Describe the mission for this workspace"></textarea>
                        </label>
                    </div>
                {:else if selectedKind === 'folder'}
                    <div class="field-stack">
                        <label>
                            <span>Folder</span>
                            <div class="folder-row">
                                <input bind:value={selectedFolder} placeholder="Choose a working directory" readonly />
                                <button class="secondary-button" type="button" onclick={chooseFolder}>Browse</button>
                            </div>
                        </label>
                    </div>
                {:else}
                    <div class="details-note">
                        <strong>Shell-first by default</strong>
                        <p>This creates plain shell slots. Provider assignment can be added later as an advanced setup layer.</p>
                    </div>
                {/if}

                <div class="footer-actions">
                    <button class="secondary-button" type="button" onclick={() => (step = 'layout')}>Back</button>
                    <button class="primary-button" type="button" onclick={handleCreate} disabled={!canCreate() || creating}>
                        {creating ? 'Creating...' : 'Create workspace'}
                    </button>
                </div>
            </div>
        {/if}
    </div>
</div>

<style>
    .launcher-backdrop {
        position: fixed;
        inset: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 24px;
        background: rgba(5, 7, 12, 0.72);
        z-index: 1200;
    }

    .launcher-shell {
        width: min(860px, 100%);
        max-height: min(760px, calc(100dvh - 48px));
        overflow: auto;
        border: 1px solid var(--ui-border-soft);
        border-radius: 12px;
        background: var(--ui-layer-1);
        color: var(--ui-text-primary);
        box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    }

    .launcher-header {
        display: flex;
        justify-content: space-between;
        gap: 20px;
        padding: 18px 18px 14px;
        border-bottom: 1px solid var(--ui-border-soft);
    }

    .eyebrow {
        margin: 0 0 4px;
        font-size: 0.72rem;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        color: var(--ui-accent);
    }

    .launcher-header h2,
    .selection-summary strong,
    .option-card strong,
    .layout-card strong,
    .details-note strong {
        margin: 0;
        font-size: 0.98rem;
        font-weight: 600;
    }

    .subcopy,
    .option-card span,
    .option-card small,
    .layout-card span,
    .details-note p,
    .selection-summary p {
        margin: 0;
        color: var(--ui-text-secondary);
        line-height: 1.45;
    }

    .close-button,
    .option-card,
    .layout-card,
    .primary-button,
    .secondary-button,
    input,
    textarea {
        font: inherit;
    }

    .close-button,
    .primary-button,
    .secondary-button {
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        padding: 8px 10px;
        cursor: pointer;
    }

    .close-button:hover,
    .option-card:hover,
    .layout-card:hover,
    .primary-button:hover,
    .secondary-button:hover {
        border-color: var(--ui-border-strong);
    }

    .primary-button {
        background: color-mix(in srgb, var(--ui-accent) 14%, var(--ui-layer-2) 86%);
        border-color: color-mix(in srgb, var(--ui-accent) 24%, transparent);
    }

    .primary-button:disabled {
        opacity: 0.55;
        cursor: not-allowed;
    }

    .step-strip {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        padding: 14px 18px 0;
    }

    .step-chip,
    .step-count {
        font-size: 0.74rem;
        color: var(--ui-text-secondary);
        white-space: nowrap;
    }

    .step-chip {
        color: var(--ui-accent);
        font-weight: 600;
        letter-spacing: 0.04em;
        text-transform: uppercase;
    }

    .step-track {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 6px;
        flex: 1;
    }

    .step-track span {
        display: block;
        height: 4px;
        border-radius: 999px;
        background: var(--ui-border-soft);
    }

    .step-track span.active {
        background: var(--ui-accent);
    }

    .card-grid,
    .details-view {
        padding: 18px;
    }

    .kind-grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 12px;
    }

    .layout-grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 12px;
    }

    .option-card,
    .layout-card {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 8px;
        min-width: 0;
        padding: 14px;
        text-align: left;
        border: 1px solid var(--ui-border-soft);
        border-radius: 10px;
        background: var(--ui-layer-2);
        color: inherit;
        cursor: pointer;
    }

    .option-card small {
        color: var(--ui-text-muted);
    }

    .layout-preview {
        display: grid;
        gap: 4px;
        width: 100%;
        min-height: 54px;
        align-content: start;
    }

    .layout-preview.layout-single {
        grid-template-columns: 54px;
    }

    .layout-preview.layout-pair,
    .layout-preview.layout-shell_browser {
        grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .layout-preview.layout-quad {
        grid-template-columns: repeat(2, minmax(0, 1fr));
    }

    .layout-preview.layout-six {
        grid-template-columns: repeat(3, minmax(0, 1fr));
    }

    .layout-preview.layout-eight {
        grid-template-columns: repeat(4, minmax(0, 1fr));
    }

    .layout-preview span {
        min-height: 22px;
        border-radius: 4px;
        border: 1px solid var(--ui-border-soft);
        background: var(--ui-layer-0);
    }

    .layout-preview.browser-mix span.browser {
        background: color-mix(in srgb, var(--ui-accent) 10%, var(--ui-layer-0) 90%);
    }

    .details-view {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }

    .selection-summary {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px;
    }

    .selection-summary > div,
    .details-note {
        padding: 12px;
        border: 1px solid var(--ui-border-soft);
        border-radius: 10px;
        background: var(--ui-layer-2);
    }

    .field-stack {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .field-stack label {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .field-stack span {
        color: var(--ui-text-secondary);
        font-size: 0.8rem;
        font-weight: 600;
    }

    input,
    textarea {
        width: 100%;
        box-sizing: border-box;
        border: 1px solid var(--ui-border-soft);
        border-radius: 8px;
        background: var(--ui-layer-0);
        color: var(--ui-text-primary);
        padding: 10px;
    }

    .folder-row {
        display: flex;
        gap: 10px;
    }

    .folder-row input {
        flex: 1;
    }

    .footer-actions {
        display: flex;
        justify-content: flex-end;
        gap: 10px;
    }

    @media (max-width: 820px) {
        .kind-grid,
        .layout-grid,
        .selection-summary {
            grid-template-columns: 1fr;
        }
    }
</style>
