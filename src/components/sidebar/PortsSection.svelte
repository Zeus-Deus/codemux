<script lang="ts">
    import { appState } from '../../stores/core';
    import { createTab, killPort } from '../../stores/workspace';
    import { browserOpenUrl } from '../../stores/browser';
    import type { PortInfoSnapshot } from '../../stores/types';

    let expanded = $state(false);
    let confirmingKill = $state<number | null>(null);

    const ports = $derived($appState?.detected_ports ?? []);
    const portCount = $derived(ports.length);

    interface WorkspaceGroup {
        workspaceId: string | null;
        workspaceName: string;
        ports: PortInfoSnapshot[];
    }

    const grouped = $derived.by(() => {
        const groups = new Map<string, WorkspaceGroup>();
        for (const port of ports) {
            const key = port.workspace_id ?? '__unassigned__';
            if (!groups.has(key)) {
                const workspace = $appState?.workspaces.find(
                    (w) => w.workspace_id === port.workspace_id
                );
                groups.set(key, {
                    workspaceId: port.workspace_id,
                    workspaceName: workspace?.title ?? 'Unassigned',
                    ports: [],
                });
            }
            groups.get(key)!.ports.push(port);
        }
        return Array.from(groups.values());
    });

    async function openInBrowser(port: PortInfoSnapshot) {
        const workspaceId =
            port.workspace_id ?? $appState?.active_workspace_id;
        if (!workspaceId) return;

        try {
            await createTab(workspaceId, 'browser');
            // After state update, find the newly created browser tab
            await new Promise((r) => setTimeout(r, 100));
            const workspace = $appState?.workspaces.find(
                (w) => w.workspace_id === workspaceId
            );
            const browserTab = workspace?.tabs
                .filter((t) => t.kind === 'browser' && t.browser_id)
                .at(-1);
            if (browserTab?.browser_id) {
                await browserOpenUrl(
                    browserTab.browser_id,
                    `http://localhost:${port.port}`
                );
            }
        } catch (error) {
            console.error('Failed to open port in browser:', error);
        }
    }

    async function handleKill(port: number) {
        if (confirmingKill === port) {
            try {
                await killPort(port);
            } catch (error) {
                console.error('Failed to kill port:', error);
            }
            confirmingKill = null;
        } else {
            confirmingKill = port;
            setTimeout(() => {
                if (confirmingKill === port) confirmingKill = null;
            }, 3000);
        }
    }
</script>

<div class="section">
    <button
        class="section-header"
        type="button"
        onclick={() => (expanded = !expanded)}
    >
        <span class="section-label">Ports</span>
        {#if portCount > 0}
            <span class="port-badge">{portCount}</span>
        {/if}
        <span class="spacer"></span>
        <span class="chevron" class:open={expanded}>›</span>
    </button>

    {#if expanded}
        <div class="section-body">
            {#each grouped as group}
                {#if grouped.length > 1}
                    <div class="group-label">{group.workspaceName}</div>
                {/if}
                <div class="port-list">
                    {#each group.ports as port (port.port)}
                        <div class="port-pill">
                            <span class="port-info">
                                {#if port.label}
                                    <span class="port-label">{port.label}</span>
                                {/if}
                                <span class="port-number">{port.port}</span>
                            </span>
                            <span class="port-actions">
                                <button
                                    class="port-action-btn open-btn"
                                    type="button"
                                    title="Open in browser"
                                    onclick={() => openInBrowser(port)}
                                >
                                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                                        <circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="2"/>
                                        <path d="M2 12h20M12 2c-3 3-4.5 7-4.5 10s1.5 7 4.5 10c3-3 4.5-7 4.5-10S15 5 12 2z" stroke="currentColor" stroke-width="2"/>
                                    </svg>
                                </button>
                                <button
                                    class="port-action-btn kill-btn"
                                    class:confirming={confirmingKill === port.port}
                                    type="button"
                                    title={confirmingKill === port.port ? 'Click again to confirm' : `Kill ${port.process_name} (PID ${port.pid})`}
                                    onclick={() => handleKill(port.port)}
                                >
                                    {#if confirmingKill === port.port}
                                        <span class="confirm-text">kill?</span>
                                    {:else}
                                        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                            <path d="M2 2l6 6M8 2l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                                        </svg>
                                    {/if}
                                </button>
                            </span>
                        </div>
                    {/each}
                </div>
            {/each}
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

    .port-badge {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-width: 18px;
        height: 18px;
        padding: 0 5px;
        border-radius: 9px;
        background: var(--ui-accent);
        color: #0d1117;
        font-size: 0.66rem;
        font-weight: 700;
        flex-shrink: 0;
    }

    .spacer {
        flex: 1;
    }

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
        gap: 4px;
        padding: 2px 8px 8px;
    }

    .group-label {
        font-size: 0.68rem;
        color: var(--ui-text-muted);
        text-transform: uppercase;
        letter-spacing: 0.03em;
        padding: 4px 4px 2px;
    }

    .port-list {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }

    .port-pill {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        background: color-mix(in srgb, var(--ui-accent) 10%, transparent);
        border-radius: 6px;
        padding: 2px 4px 2px 8px;
        transition: background var(--ui-motion-fast);
    }

    .port-pill:hover {
        background: color-mix(in srgb, var(--ui-accent) 18%, transparent);
    }

    .port-info {
        display: inline-flex;
        align-items: center;
        gap: 4px;
    }

    .port-label {
        font-size: 0.72rem;
        color: var(--ui-text-secondary);
    }

    .port-number {
        font-family: var(--ui-font-mono);
        font-size: 0.75rem;
        color: var(--ui-text-primary);
        font-weight: 600;
    }

    .port-actions {
        display: inline-flex;
        align-items: center;
        gap: 1px;
        opacity: 0;
        transition: opacity var(--ui-motion-fast);
    }

    .port-pill:hover .port-actions {
        opacity: 1;
    }

    .port-action-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 20px;
        height: 20px;
        background: transparent;
        border: none;
        border-radius: 4px;
        color: var(--ui-text-muted);
        cursor: pointer;
        padding: 0;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast);
    }

    .open-btn:hover {
        background: color-mix(in srgb, var(--ui-accent) 20%, transparent);
        color: var(--ui-accent);
    }

    .kill-btn:hover,
    .kill-btn.confirming {
        background: color-mix(in srgb, var(--ui-danger) 15%, transparent);
        color: var(--ui-danger);
    }

    .confirm-text {
        font-size: 0.6rem;
        font-weight: 700;
        white-space: nowrap;
    }
</style>
