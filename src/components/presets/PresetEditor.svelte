<script lang="ts">
    import { createEventDispatcher } from 'svelte';
    import type { TerminalPreset, LaunchMode } from '../../stores/types';
    import { createPreset, updatePreset, deletePreset } from '../../stores/presets';

    const props = $props<{ preset: TerminalPreset | null }>();

    const dispatch = createEventDispatcher<{ close: void }>();

    // Capture initial values from the preset prop (editor opens once and closes).
    // These are read once at creation time — the preset prop does not change while the editor is open.
    const presetId = props.preset?.id ?? null;
    const isEdit = props.preset !== null;
    const isBuiltin = props.preset?.is_builtin ?? false;

    let name = $state(props.preset?.name ?? '');
    let description = $state(props.preset?.description ?? '');
    let commandsText = $state(props.preset?.commands.join('\n') ?? '');
    let workingDirectory = $state(props.preset?.working_directory ?? '');
    let launchMode = $state<LaunchMode>(props.preset?.launch_mode ?? 'new_tab');
    let pinned = $state(props.preset?.pinned ?? true);
    let saving = $state(false);

    async function handleSave() {
        if (!name.trim()) return;
        saving = true;

        try {
            const commands = commandsText
                .split('\n')
                .map((c: string) => c.trim())
                .filter((c: string) => c.length > 0);

            if (isEdit && presetId) {
                await updatePreset(presetId, {
                    name: name.trim(),
                    description: description.trim() || undefined,
                    commands,
                    working_directory: workingDirectory.trim() || undefined,
                    launch_mode: launchMode,
                    pinned,
                });
            } else {
                await createPreset({
                    name: name.trim(),
                    description: description.trim() || undefined,
                    commands,
                    working_directory: workingDirectory.trim() || undefined,
                    launch_mode: launchMode,
                    pinned,
                });
            }
            dispatch('close');
        } catch (error) {
            console.error('Failed to save preset:', error);
        } finally {
            saving = false;
        }
    }

    async function handleDelete() {
        if (!presetId || isBuiltin) return;
        try {
            await deletePreset(presetId);
            dispatch('close');
        } catch (error) {
            console.error('Failed to delete preset:', error);
        }
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === 'Escape') dispatch('close');
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="editor-backdrop" onclick={() => dispatch('close')} onkeydown={handleKeydown}>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="editor-shell" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
        <div class="editor-header">
            <h2>{isEdit ? 'Edit Preset' : 'New Preset'}</h2>
        </div>

        <div class="editor-body">
            <label class="field">
                <span class="field-label">Name</span>
                <input type="text" class="field-input" bind:value={name} placeholder="My preset" />
            </label>

            <label class="field">
                <span class="field-label">Description <span class="optional">(optional)</span></span>
                <input type="text" class="field-input" bind:value={description} placeholder="What this preset does" disabled={isBuiltin} />
            </label>

            <label class="field">
                <span class="field-label">Commands <span class="optional">(one per line)</span></span>
                <textarea class="field-textarea" bind:value={commandsText} rows="4" placeholder="npm run dev" disabled={isBuiltin}></textarea>
            </label>

            <label class="field">
                <span class="field-label">Working directory <span class="optional">(optional, relative to workspace)</span></span>
                <input type="text" class="field-input" bind:value={workingDirectory} placeholder="workspace default" disabled={isBuiltin} />
            </label>

            <div class="field">
                <span class="field-label">Launch mode</span>
                <div class="toggle-group">
                    <button
                        class="toggle-btn"
                        class:active={launchMode === 'new_tab'}
                        onclick={() => { launchMode = 'new_tab'; }}
                        disabled={isBuiltin}
                    >New Tab</button>
                    <button
                        class="toggle-btn"
                        class:active={launchMode === 'split_pane'}
                        onclick={() => { launchMode = 'split_pane'; }}
                        disabled={isBuiltin}
                    >Split Pane</button>
                </div>
            </div>

            <label class="field field-row">
                <input type="checkbox" bind:checked={pinned} />
                <span class="field-label">Pin to preset bar</span>
            </label>
        </div>

        <div class="editor-footer">
            {#if isEdit && !isBuiltin}
                <button class="btn btn-danger" onclick={handleDelete}>Delete</button>
            {/if}
            <div class="footer-spacer"></div>
            <button class="btn btn-secondary" onclick={() => dispatch('close')}>Cancel</button>
            <button class="btn btn-primary" onclick={handleSave} disabled={saving || !name.trim() || isBuiltin}>
                {saving ? 'Saving...' : 'Save'}
            </button>
        </div>
    </div>
</div>

<style>
    .editor-backdrop {
        position: fixed;
        inset: 0;
        z-index: 100;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.5);
    }

    .editor-shell {
        width: min(480px, calc(100vw - 48px));
        max-height: min(600px, calc(100dvh - 48px));
        overflow-y: auto;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-lg);
        box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    }

    .editor-header {
        padding: 16px 20px 8px;
    }

    .editor-header h2 {
        margin: 0;
        font-size: 1rem;
        font-weight: 600;
        font-family: var(--ui-font-sans);
        color: var(--ui-text-primary);
    }

    .editor-body {
        display: flex;
        flex-direction: column;
        gap: 12px;
        padding: 8px 20px 16px;
    }

    .field {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .field-row {
        flex-direction: row;
        align-items: center;
        gap: 8px;
    }

    .field-label {
        font-size: 0.78rem;
        font-family: var(--ui-font-sans);
        color: var(--ui-text-secondary);
    }

    .optional {
        color: var(--ui-text-muted);
    }

    .field-input, .field-textarea {
        padding: 6px 10px;
        background: var(--ui-layer-1);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-primary);
        font-size: 0.82rem;
        font-family: var(--ui-font-mono);
        outline: none;
        transition: border-color var(--ui-motion-fast);
    }

    .field-input:focus, .field-textarea:focus {
        border-color: var(--ui-accent);
    }

    .field-input:disabled, .field-textarea:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .field-textarea {
        resize: vertical;
        min-height: 60px;
    }

    .toggle-group {
        display: flex;
        gap: 0;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        overflow: hidden;
    }

    .toggle-btn {
        flex: 1;
        padding: 5px 12px;
        border: none;
        background: transparent;
        color: var(--ui-text-secondary);
        font-size: 0.78rem;
        font-family: var(--ui-font-sans);
        cursor: pointer;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .toggle-btn:not(:last-child) {
        border-right: 1px solid var(--ui-border-soft);
    }

    .toggle-btn.active {
        background: var(--ui-accent-soft);
        color: var(--ui-accent);
    }

    .toggle-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .editor-footer {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 12px 20px;
        border-top: 1px solid var(--ui-border-soft);
    }

    .footer-spacer {
        flex: 1;
    }

    .btn {
        padding: 6px 16px;
        border: none;
        border-radius: var(--ui-radius-sm);
        font-size: 0.82rem;
        font-family: var(--ui-font-sans);
        cursor: pointer;
        transition: background var(--ui-motion-fast), opacity var(--ui-motion-fast);
    }

    .btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .btn-primary {
        background: var(--ui-accent);
        color: #fff;
    }

    .btn-primary:hover:not(:disabled) {
        opacity: 0.9;
    }

    .btn-secondary {
        background: var(--ui-layer-3);
        color: var(--ui-text-secondary);
    }

    .btn-secondary:hover:not(:disabled) {
        color: var(--ui-text-primary);
    }

    .btn-danger {
        background: transparent;
        color: var(--ui-danger);
        border: 1px solid color-mix(in srgb, var(--ui-danger) 30%, transparent);
    }

    .btn-danger:hover:not(:disabled) {
        background: color-mix(in srgb, var(--ui-danger) 12%, transparent);
    }

    input[type="checkbox"] {
        accent-color: var(--ui-accent);
    }
</style>
