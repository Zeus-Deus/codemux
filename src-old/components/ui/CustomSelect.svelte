<script lang="ts">
    interface Option {
        value: string;
        label: string;
        disabled?: boolean;
    }

    let {
        options,
        value = $bindable(''),
        placeholder = 'Select…',
        disabled = false,
        id = undefined,
        compact = false,
        onchange = undefined,
    }: {
        options: Option[];
        value: string;
        placeholder?: string;
        disabled?: boolean;
        id?: string;
        compact?: boolean;
        onchange?: (value: string) => void;
    } = $props();

    let open = $state(false);
    let triggerEl: HTMLButtonElement | undefined = $state();
    let contentEl: HTMLDivElement | undefined = $state();
    let focusedIndex = $state(-1);

    const selectedLabel = $derived(
        options.find(o => o.value === value)?.label ?? placeholder
    );

    const isPlaceholder = $derived(!options.some(o => o.value === value));

    function toggle() {
        if (disabled) return;
        open = !open;
        if (open) focusedIndex = options.findIndex(o => o.value === value);
    }

    function select(opt: Option) {
        if (opt.disabled) return;
        value = opt.value;
        onchange?.(opt.value);
        open = false;
        triggerEl?.focus();
    }

    function handleKeydown(e: KeyboardEvent) {
        if (!open) {
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                open = true;
                focusedIndex = options.findIndex(o => o.value === value);
            }
            return;
        }

        switch (e.key) {
            case 'Escape':
                e.preventDefault();
                open = false;
                triggerEl?.focus();
                break;
            case 'ArrowDown':
                e.preventDefault();
                focusedIndex = nextEnabled(focusedIndex, 1);
                break;
            case 'ArrowUp':
                e.preventDefault();
                focusedIndex = nextEnabled(focusedIndex, -1);
                break;
            case 'Enter':
            case ' ':
                e.preventDefault();
                if (focusedIndex >= 0 && focusedIndex < options.length) {
                    select(options[focusedIndex]);
                }
                break;
        }
    }

    function nextEnabled(from: number, dir: number): number {
        let idx = from;
        for (let i = 0; i < options.length; i++) {
            idx = (idx + dir + options.length) % options.length;
            if (!options[idx].disabled) return idx;
        }
        return from;
    }

    function handleClickOutside(e: MouseEvent) {
        if (!open) return;
        const target = e.target as Node;
        if (triggerEl?.contains(target) || contentEl?.contains(target)) return;
        open = false;
    }
</script>

<svelte:window onclick={handleClickOutside} />

<div class="select-wrapper" class:compact>
    <button
        type="button"
        class="select-trigger"
        class:open
        class:placeholder={isPlaceholder}
        {disabled}
        {id}
        bind:this={triggerEl}
        onclick={toggle}
        onkeydown={handleKeydown}
        aria-haspopup="listbox"
        aria-expanded={open}
    >
        <span class="select-label">{selectedLabel}</span>
        <svg class="select-chevron" width="12" height="12" viewBox="0 0 12 12" fill="none">
            <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
    </button>

    {#if open}
        <div class="select-content" bind:this={contentEl} role="listbox">
            {#each options as opt, i}
                <button
                    type="button"
                    class="select-item"
                    class:selected={opt.value === value}
                    class:focused={i === focusedIndex}
                    class:disabled={opt.disabled}
                    disabled={opt.disabled}
                    role="option"
                    aria-selected={opt.value === value}
                    onclick={() => select(opt)}
                    onmouseenter={() => { if (!opt.disabled) focusedIndex = i; }}
                >
                    <span class="item-label">{opt.label}</span>
                    {#if opt.value === value}
                        <svg class="item-check" width="12" height="12" viewBox="0 0 12 12" fill="none">
                            <path d="M2.5 6L5 8.5L9.5 3.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                        </svg>
                    {/if}
                </button>
            {/each}
        </div>
    {/if}
</div>

<style>
    .select-wrapper {
        position: relative;
        width: 100%;
    }

    .select-trigger {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        width: 100%;
        min-height: 36px;
        padding: 6px 12px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-md);
        color: var(--ui-text-primary);
        font: inherit;
        font-size: 0.85rem;
        cursor: pointer;
        box-shadow: var(--ui-shadow-xs);
        transition: border-color var(--ui-motion-fast), box-shadow var(--ui-motion-fast);
        text-align: left;
    }

    .compact .select-trigger {
        min-height: 32px;
        padding: 4px 10px;
        font-size: 0.75rem;
    }

    .select-trigger:hover:not(:disabled) {
        border-color: var(--ui-border-strong);
    }

    .select-trigger:focus-visible {
        outline: none;
        border-color: color-mix(in srgb, var(--ui-accent) 50%, transparent);
        box-shadow: 0 0 0 3px var(--ui-ring-color);
    }

    .select-trigger:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .select-trigger.placeholder {
        color: var(--ui-text-muted);
    }

    .select-label {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .select-chevron {
        flex-shrink: 0;
        color: var(--ui-text-muted);
        transition: transform var(--ui-motion-fast);
    }

    .select-trigger.open .select-chevron {
        transform: rotate(180deg);
    }

    .select-content {
        position: absolute;
        top: calc(100% + 4px);
        left: 0;
        right: 0;
        z-index: 50;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: var(--ui-shadow-md);
        padding: 4px;
        max-height: 200px;
        overflow-y: auto;
    }

    .select-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
        width: 100%;
        padding: 6px 8px;
        border: none;
        border-radius: var(--ui-radius-sm);
        background: transparent;
        color: var(--ui-text-secondary);
        font: inherit;
        font-size: 0.85rem;
        cursor: pointer;
        text-align: left;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast);
    }

    .compact .select-item {
        padding: 5px 8px;
        font-size: 0.75rem;
    }

    .select-item:hover:not(:disabled),
    .select-item.focused:not(:disabled) {
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
        color: var(--ui-text-primary);
    }

    .select-item.selected {
        color: var(--ui-accent);
    }

    .select-item.disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }

    .item-label {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .item-check {
        flex-shrink: 0;
        color: var(--ui-accent);
    }
</style>
