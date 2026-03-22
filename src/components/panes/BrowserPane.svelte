<script lang="ts">
    import { onMount, onDestroy, tick } from 'svelte';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { appState } from '../../stores/core';
    import {
        browserSpawn,
        browserNavigate,
        browserOpenUrl,
        browserResizeViewport,
        browserSetLoadingState,
        browserScreenshot,
        browserClick,
        agentBrowserSpawn,
        agentBrowserRun,
        agentBrowserScreenshot,
    } from '../../stores/browser';

    let { browserId }: { browserId: string } = $props();

    let address = $state('about:blank');
    let screenshotData = $state<string | null>(null);
    let isLoading = $state(false);
    let errorMessage = $state<string | null>(null);
    let browserReady = $state(false);
    let screenshotInterval: ReturnType<typeof setInterval> | null = null;
    let screenshotInFlight = $state(false);
    let isEditingAddress = $state(false);
    let mounted = true;
    let viewportElement = $state<HTMLDivElement | null>(null);
    let resizeObserver: ResizeObserver | null = null;
    let viewportWidth = $state(1280);
    let viewportHeight = $state(720);
    
    const useAgentBrowser = true;
    const BROWSER_SESSION = 'default';

    const browser = $derived(
        $appState?.browser_sessions.find((b) => b.browser_id === browserId) ?? null
    );

    $effect(() => {
        const currentUrl = browser?.current_url?.trim();
        if (currentUrl && currentUrl !== address && !isLoading && !isEditingAddress) {
            address = currentUrl;
        }

        if (browser?.last_error && browser.last_error !== errorMessage) {
            errorMessage = browser.last_error;
        }
    });

    async function initBrowser() {
        try {
            isLoading = true;
            errorMessage = null;
            
            if (useAgentBrowser) {
                await agentBrowserSpawn(BROWSER_SESSION);
                browserReady = true;
                await tick();
                await syncViewportSize();
                
                screenshotInterval = setInterval(() => {
                    void refreshScreenshot();
                }, 1000);
                
                isLoading = false;
                return;
            }

            await browserSpawn(browserId);

            browserReady = true;
            await tick();
            await syncViewportSize();

            const initialUrl = browser?.current_url?.trim() || address.trim() || 'about:blank';
            address = initialUrl;

            if (initialUrl !== 'about:blank') {
                await navigate(initialUrl, false);
            } else {
                await refreshScreenshot();
                await browserSetLoadingState(browserId, false, null);
                isLoading = false;
            }

            screenshotInterval = setInterval(() => {
                void refreshScreenshot();
            }, 1000);
        } catch (e) {
            errorMessage = e instanceof Error ? e.message : String(e);
            await browserSetLoadingState(browserId, false, errorMessage).catch(() => {});
            isLoading = false;
        }
    }

    async function navigate(url = address, syncState = true) {
        const nextUrl = url.trim();
        if (!nextUrl) return;

        try {
            isLoading = true;
            errorMessage = null;

            if (useAgentBrowser) {
                await agentBrowserRun(BROWSER_SESSION, 'open', { url: nextUrl });
                address = nextUrl;
                await refreshScreenshot();
                isLoading = false;
                return;
            }

            if (syncState) {
                await browserOpenUrl(browserId, nextUrl);
            }

            await browserSetLoadingState(browserId, true, null);
            await browserNavigate(browserId, nextUrl);
            address = nextUrl;
            await refreshScreenshot();
            await browserSetLoadingState(browserId, false, null);
        } catch (e) {
            errorMessage = e instanceof Error ? e.message : String(e);
            if (!useAgentBrowser) {
                await browserSetLoadingState(browserId, false, errorMessage).catch(() => {});
            }
        } finally {
            isLoading = false;
        }
    }

    async function handleClick(x: number, y: number) {
        try {
            if (useAgentBrowser) {
                await agentBrowserRun(BROWSER_SESSION, 'click', { selector: 'body' });
            } else {
                await browserClick(browserId, x, y);
            }
            await refreshScreenshot();
        } catch (e) {
            errorMessage = e instanceof Error ? e.message : String(e);
        }
    }

    function clickCoordinates(event: MouseEvent) {
        const viewport = event.currentTarget as HTMLElement;
        const rect = viewport.getBoundingClientRect();
        const renderedAspect = rect.width / rect.height;
        const browserAspect = viewportWidth / viewportHeight;

        let contentWidth = rect.width;
        let contentHeight = rect.height;
        let offsetX = 0;
        let offsetY = 0;

        if (renderedAspect > browserAspect) {
            contentWidth = rect.height * browserAspect;
            offsetX = (rect.width - contentWidth) / 2;
        } else {
            contentHeight = rect.width / browserAspect;
            offsetY = (rect.height - contentHeight) / 2;
        }

        const localX = Math.min(Math.max(event.clientX - rect.left - offsetX, 0), contentWidth);
        const localY = Math.min(Math.max(event.clientY - rect.top - offsetY, 0), contentHeight);

        return {
            x: (localX / contentWidth) * viewportWidth,
            y: (localY / contentHeight) * viewportHeight,
        };
    }

    async function handleKeydown(event: KeyboardEvent) {
        if (event.key === 'Enter') {
            void navigate();
        }
    }

    function handleAddressFocus() {
        isEditingAddress = true;
    }

    function handleAddressBlur() {
        isEditingAddress = false;
        const currentUrl = browser?.current_url?.trim();
        if (currentUrl && !isLoading) {
            address = currentUrl;
        }
    }

    function initialHintText() {
        if (errorMessage) {
            return 'Browser ready, but the last navigation failed.';
        }

        if (address === 'about:blank') {
            return 'Type a URL like google.com, then press Enter.';
        }

        return 'Browser initializing...';
    }

    async function refreshScreenshot() {
        if (!browserReady || screenshotInFlight || !mounted) {
            return;
        }

        try {
            screenshotInFlight = true;
            
            if (useAgentBrowser) {
                const data = await agentBrowserScreenshot(BROWSER_SESSION);
                if (mounted && data) {
                    screenshotData = data.startsWith('data:') ? data : `data:image/png;base64,${data}`;
                    errorMessage = null;
                }
                screenshotInFlight = false;
                return;
            }
            
            const data = await browserScreenshot(browserId);
            if (mounted) {
                screenshotData = data;
                errorMessage = null;
            }
        } catch (e) {
            if (mounted) {
                errorMessage = e instanceof Error ? e.message : String(e);
            }
        } finally {
            screenshotInFlight = false;
        }
    }

    async function syncViewportSize() {
        if (!viewportElement || !browserReady) {
            return;
        }

        const rect = viewportElement.getBoundingClientRect();
        const nextWidth = Math.max(320, Math.round(rect.width));
        const nextHeight = Math.max(240, Math.round(rect.height));

        if (nextWidth === viewportWidth && nextHeight === viewportHeight) {
            return;
        }

        viewportWidth = nextWidth;
        viewportHeight = nextHeight;

        try {
            await browserResizeViewport(browserId, nextWidth, nextHeight);
            await refreshScreenshot();
        } catch (e) {
            errorMessage = e instanceof Error ? e.message : String(e);
        }
    }

    async function openExternal() {
        if (!address.trim()) return;
        await openUrl(address.trim());
    }

    onMount(() => {
        mounted = true;
        resizeObserver = new ResizeObserver(() => {
            void syncViewportSize();
        });

        void initBrowser();
    });

    $effect(() => {
        if (!resizeObserver || !viewportElement) {
            return;
        }

        resizeObserver.disconnect();
        resizeObserver.observe(viewportElement);
        void syncViewportSize();

        return () => {
            resizeObserver?.disconnect();
        };
    });

    onDestroy(() => {
        mounted = false;
        if (screenshotInterval) {
            clearInterval(screenshotInterval);
        }
        resizeObserver?.disconnect();
        resizeObserver = null;
    });
</script>

<section class="browser-shell">
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <header class="browser-toolbar" onclick={(event) => event.stopPropagation()}>
        <div class="nav-buttons">
            <button class="nav-btn" type="button" onclick={() => void navigate('about:blank')} title="Home" aria-label="Go home">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M6 1L1 5.5V11h4V7h2v4h4V5.5L6 1z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
                </svg>
            </button>
            <button class="nav-btn" type="button" onclick={refreshScreenshot} title="Refresh" aria-label="Refresh screenshot">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M10 6A4 4 0 1 1 8 2.5L10 2v3h-3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
        </div>

        <form class="address-form" onsubmit={(e) => { e.preventDefault(); void navigate(); }}>
            <div class="address-bar" class:loading={isLoading}>
                {#if isLoading}
                    <span class="addr-spinner"></span>
                {:else}
                    <svg class="addr-icon" width="11" height="11" viewBox="0 0 11 11" fill="none" aria-hidden="true">
                        <rect x="1" y="2" width="9" height="7" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                        <path d="M1 4h9" stroke="currentColor" stroke-width="1.2"/>
                    </svg>
                {/if}
                <input
                    class="address-input"
                    bind:value={address}
                    placeholder="Enter URL"
                    spellcheck="false"
                    autocomplete="off"
                    onfocus={handleAddressFocus}
                    onblur={handleAddressBlur}
                    onkeydown={handleKeydown}
                />
            </div>
        </form>

        <div class="util-buttons">
            <button class="nav-btn" type="button" onclick={openExternal} title="Open in external browser" aria-label="Open externally">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M5 2H2.5A1.5 1.5 0 0 0 1 3.5v6A1.5 1.5 0 0 0 2.5 11h6A1.5 1.5 0 0 0 10 9.5V7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                    <path d="M7 1h4v4M11 1L6 6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
        </div>
    </header>

    <div bind:this={viewportElement} class="browser-body">
        {#if errorMessage && !screenshotData}
            <div class="browser-overlay error">
                <h3>Error</h3>
                <p>{errorMessage}</p>
            </div>
        {/if}

        {#if errorMessage && screenshotData}
            <div class="browser-status-banner error" aria-live="polite">
                <strong>Error:</strong>
                <span>{errorMessage}</span>
            </div>
        {/if}

        {#if isLoading && !screenshotData}
            <div class="browser-loading">
                <span class="addr-spinner"></span>
                <p>Starting browser...</p>
            </div>
        {:else if screenshotData}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="browser-viewport" onclick={(e) => {
                const coords = clickCoordinates(e);
                void handleClick(coords.x, coords.y);
            }}>
                <img src={screenshotData} alt="Browser content" />
            </div>
        {:else}
            <div class="browser-empty">
                <p>{initialHintText()}</p>
            </div>
        {/if}

        <div class="browser-hint">
            <p>Click on the browser viewport to interact. Screenshot refreshes every second.</p>
        </div>
    </div>
</section>

<style>
    .browser-shell {
        display: flex;
        flex-direction: column;
        flex: 1;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        background: var(--ui-layer-0);
    }

    .browser-toolbar {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 5px 8px;
        background: color-mix(in srgb, var(--ui-layer-1) 80%, transparent 20%);
        border-bottom: 1px solid var(--ui-border-soft);
        flex: 0 0 auto;
    }

    .nav-buttons,
    .util-buttons {
        display: flex;
        gap: 2px;
        flex-shrink: 0;
    }

    .nav-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 26px;
        height: 26px;
        background: transparent;
        border: 1px solid transparent;
        border-radius: var(--ui-radius-sm);
        color: var(--ui-text-muted);
        cursor: pointer;
        font: inherit;
        transition: background var(--ui-motion-fast), color var(--ui-motion-fast), border-color var(--ui-motion-fast);
        padding: 0;
    }

    .nav-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        border-color: var(--ui-border-soft);
    }

    .address-form {
        flex: 1;
        min-width: 0;
    }

    .address-bar {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0 8px;
        height: 26px;
        background: var(--ui-layer-0);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-sm);
        transition: border-color var(--ui-motion-fast), background var(--ui-motion-fast);
    }

    .address-bar:focus-within {
        border-color: color-mix(in srgb, var(--ui-accent) 36%, transparent);
        background: var(--ui-layer-1);
    }

    .addr-icon {
        color: var(--ui-text-muted);
        flex-shrink: 0;
    }

    .addr-spinner {
        width: 10px;
        height: 10px;
        border: 1.5px solid var(--ui-border-soft);
        border-top-color: var(--ui-accent);
        border-radius: 50%;
        flex-shrink: 0;
        animation: spin 0.8s linear infinite;
    }

    @keyframes spin {
        to { transform: rotate(360deg); }
    }

    .address-input {
        flex: 1;
        min-width: 0;
        background: transparent;
        border: none;
        color: var(--ui-text-primary);
        font-family: var(--ui-font-mono);
        font-size: 0.78rem;
        padding: 0;
        outline: none;
    }

    .address-input::placeholder {
        color: var(--ui-text-muted);
    }

    .browser-body {
        position: relative;
        flex: 1;
        width: 100%;
        min-height: 0;
        overflow: hidden;
        background: white;
    }

    .browser-viewport {
        width: 100%;
        height: 100%;
        cursor: crosshair;
    }

    .browser-viewport img {
        width: 100%;
        height: 100%;
        object-fit: contain;
        background: white;
    }

    .browser-overlay {
        position: absolute;
        inset: 16px;
        z-index: 10;
        padding: 16px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-lg);
        background: var(--ui-layer-1);
    }

    .browser-overlay.error {
        border-color: color-mix(in srgb, var(--ui-danger) 28%, transparent);
        background: color-mix(in srgb, var(--ui-danger) 6%, var(--ui-layer-1) 94%);
    }

    .browser-overlay h3 {
        margin: 0 0 6px;
        font-size: 0.86rem;
        font-weight: 600;
        color: var(--ui-danger);
    }

    .browser-overlay p {
        margin: 0;
        font-size: 0.78rem;
        color: var(--ui-text-secondary);
        line-height: 1.45;
    }

    .browser-status-banner {
        position: absolute;
        top: 12px;
        left: 12px;
        z-index: 8;
        display: inline-flex;
        align-items: center;
        gap: 8px;
        max-width: min(640px, calc(100% - 24px));
        padding: 8px 10px;
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-md);
        background: color-mix(in srgb, var(--ui-layer-1) 90%, transparent 10%);
        box-shadow: 0 10px 30px rgba(0, 0, 0, 0.18);
        pointer-events: none;
    }

    .browser-status-banner.error {
        border-color: color-mix(in srgb, var(--ui-danger) 28%, transparent);
        background: color-mix(in srgb, var(--ui-danger) 8%, var(--ui-layer-1) 92%);
    }

    .browser-status-banner strong,
    .browser-status-banner span {
        font-size: 0.72rem;
        line-height: 1.35;
    }

    .browser-status-banner strong {
        color: var(--ui-danger);
    }

    .browser-status-banner span {
        color: var(--ui-text-secondary);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .browser-loading,
    .browser-empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        height: 100%;
        color: var(--ui-text-muted);
    }

    .browser-loading p,
    .browser-empty p {
        margin-top: 12px;
        font-size: 0.86rem;
    }

    .browser-hint {
        position: absolute;
        bottom: 12px;
        left: 12px;
        z-index: 5;
        max-width: 320px;
        padding: 8px 10px;
        background: color-mix(in srgb, var(--ui-layer-2) 80%, transparent 20%);
        border: 1px solid var(--ui-border-soft);
        border-radius: var(--ui-radius-md);
        opacity: 0.7;
    }

    .browser-hint p {
        margin: 0;
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        line-height: 1.4;
    }
</style>
