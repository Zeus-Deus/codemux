<script lang="ts">
    let { icon, size = 14 }: { icon: string | null; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true" class="preset-icon">
    {#if icon === 'claude'}
        <!-- Lightning bolt -->
        <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
    {:else if icon === 'codex'}
        <!-- Code brackets -->
        <polyline points="16 18 22 12 16 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        <polyline points="8 6 2 12 8 18" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
    {:else if icon === 'opencode'}
        <!-- Terminal with code -->
        <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
        <path d="M6 9l3 3-3 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        <path d="M12 15h5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
    {:else if icon === 'gemini'}
        <!-- Diamond / dual star -->
        <path d="M12 2l3 7h7l-5.5 4.5 2 7L12 16l-6.5 4.5 2-7L2 9h7z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/>
    {:else if icon === 'aider'}
        <!-- Wrench -->
        <path d="M14.7 6.3a1 1 0 000 1.4l1.6 1.6a1 1 0 001.4 0l3.77-3.77a6 6 0 01-7.94 7.94l-6.91 6.91a2.12 2.12 0 01-3-3l6.91-6.91a6 6 0 017.94-7.94l-3.76 3.76z" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
    {:else}
        <!-- Terminal (default) -->
        <rect x="2" y="4" width="20" height="16" rx="3" stroke="currentColor" stroke-width="1.5"/>
        <path d="M6 9l3 3-3 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
    {/if}
</svg>

<style>
    .preset-icon {
        flex-shrink: 0;
        opacity: 0.8;
    }
</style>
