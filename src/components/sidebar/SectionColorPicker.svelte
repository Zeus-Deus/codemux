<script lang="ts">
    import { onMount } from 'svelte';

    let {
        colors,
        selected,
        position,
        onPick,
        onClose,
    }: {
        colors: readonly string[];
        selected: string;
        position: { x: number; y: number };
        onPick: (color: string) => void;
        onClose: () => void;
    } = $props();

    let el = $state<HTMLDivElement | null>(null);

    onMount(() => {
        function handleClickOutside(e: MouseEvent) {
            if (el && !el.contains(e.target as Node)) {
                onClose();
            }
        }
        window.addEventListener('mousedown', handleClickOutside);
        return () => window.removeEventListener('mousedown', handleClickOutside);
    });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="color-picker"
    bind:this={el}
    style="left: {position.x}px; top: {position.y}px;"
    onkeydown={(e) => { if (e.key === 'Escape') onClose(); }}
>
    {#each colors as color}
        <button
            class="color-swatch"
            class:selected={color === selected}
            type="button"
            style="background: {color};"
            title={color}
            onclick={(e) => { e.stopPropagation(); onPick(color); }}
        ></button>
    {/each}
</div>

<style>
    .color-picker {
        position: fixed;
        z-index: 110;
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
        padding: 8px;
        max-width: 152px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-strong);
        border-radius: var(--ui-radius-md);
        box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    }

    .color-swatch {
        width: 16px;
        height: 16px;
        border-radius: 50%;
        border: 2px solid transparent;
        cursor: pointer;
        padding: 0;
        transition: border-color var(--ui-motion-fast), transform var(--ui-motion-fast);
        flex-shrink: 0;
    }

    .color-swatch:hover {
        transform: scale(1.2);
    }

    .color-swatch.selected {
        border-color: var(--ui-text-primary);
        box-shadow: 0 0 0 1px var(--ui-layer-2);
    }
</style>
