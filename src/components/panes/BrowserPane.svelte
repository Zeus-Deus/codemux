<script lang="ts">
    import { onMount } from 'svelte';
    import { openUrl } from '@tauri-apps/plugin-opener';
    import { listen } from '@tauri-apps/api/event';
    import {
        browserAutomationComplete,
        browserAutomationRun,
        browserCaptureScreenshot,
        browserHistoryBack,
        browserHistoryForward,
        browserOpenUrl,
        browserReload,
        browserSetLoadingState,
        appState,
        type BrowserAutomationAction,
        type BrowserAutomationResult
    } from '../../stores/appState';

    let { browserId }: { browserId: string } = $props();

    let address = $state('');
    let iframeElement = $state<HTMLIFrameElement | null>(null);
    let automationLog = $state<string[]>([]);
    let automationResult = $state<string>('');

    const browser = $derived(
        $appState?.browser_sessions.find((b) => b.browser_id === browserId) ?? null
    );

    $effect(() => {
        address = browser?.current_url ?? '';
    });

    function frameWindow() {
        return iframeElement?.contentWindow ?? null;
    }

    function frameDocument() {
        return iframeElement?.contentDocument ?? null;
    }

    function ensureSameOriginDocument() {
        const doc = frameDocument();
        if (!doc) throw new Error('Browser document not available');
        return doc;
    }

    function performAutomation(action: BrowserAutomationAction): BrowserAutomationResult {
        switch (action.kind) {
            case 'open_url':
                address = action.url;
                void navigate(action.url);
                return { request_id: '', browser_id: browserId, data: { url: action.url }, message: 'Navigation requested' };
            case 'dom_snapshot': {
                const doc = ensureSameOriginDocument();
                return { request_id: '', browser_id: browserId, data: { title: doc.title, body_text: doc.body?.innerText?.slice(0, 4000) ?? '' }, message: 'DOM snapshot captured' };
            }
            case 'accessibility_snapshot': {
                const doc = ensureSameOriginDocument();
                const elements = Array.from(doc.querySelectorAll('button, a, input, textarea, select, [role]'))
                    .slice(0, 100)
                    .map((el) => ({ tag: el.tagName.toLowerCase(), role: el.getAttribute('role'), text: el.textContent?.trim() ?? '', aria_label: el.getAttribute('aria-label') }));
                return { request_id: '', browser_id: browserId, data: { elements }, message: 'Accessibility snapshot captured' };
            }
            case 'click': {
                const doc = ensureSameOriginDocument();
                const el = doc.querySelector<HTMLElement>(action.selector);
                if (!el) throw new Error(`No element matched: ${action.selector}`);
                el.click();
                return { request_id: '', browser_id: browserId, data: { selector: action.selector }, message: 'Clicked' };
            }
            case 'fill': {
                const doc = ensureSameOriginDocument();
                const el = doc.querySelector<HTMLInputElement | HTMLTextAreaElement>(action.selector);
                if (!el) throw new Error(`No fill target: ${action.selector}`);
                el.value = action.value;
                el.dispatchEvent(new Event('input', { bubbles: true }));
                el.dispatchEvent(new Event('change', { bubbles: true }));
                return { request_id: '', browser_id: browserId, data: { selector: action.selector, value: action.value }, message: 'Filled' };
            }
            case 'type_text': {
                const win = frameWindow();
                if (!win) throw new Error('Browser window not available');
                win.document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: action.text }));
                return { request_id: '', browser_id: browserId, data: { text: action.text }, message: 'Typed' };
            }
            case 'scroll': {
                const win = frameWindow();
                if (!win) throw new Error('Browser window not available');
                win.scrollBy(action.x, action.y);
                return { request_id: '', browser_id: browserId, data: { x: action.x, y: action.y }, message: 'Scrolled' };
            }
            case 'evaluate': {
                const win = frameWindow();
                if (!win) throw new Error('Browser window not available');
                const value = (win as Window & typeof globalThis & { eval: (s: string) => unknown }).eval(action.script);
                return { request_id: '', browser_id: browserId, data: { value }, message: 'Evaluated' };
            }
            case 'screenshot':
                void captureScreenshot();
                return { request_id: '', browser_id: browserId, data: { requested: true }, message: 'Screenshot requested' };
            case 'console_logs':
                return { request_id: '', browser_id: browserId, data: { logs: automationLog }, message: 'Console logs returned' };
        }
    }

    async function navigate(url = address) {
        if (!url.trim()) return;
        await browserOpenUrl(browserId, url);
        setTimeout(() => { void browserSetLoadingState(browserId, false, null); }, 300);
    }

    async function openExternal() {
        if (!address.trim()) return;
        await openUrl(address.trim());
    }

    async function goBack() {
        await browserHistoryBack(browserId);
        setTimeout(() => { void browserSetLoadingState(browserId, false, null); }, 150);
    }

    async function goForward() {
        await browserHistoryForward(browserId);
        setTimeout(() => { void browserSetLoadingState(browserId, false, null); }, 150);
    }

    async function reload() {
        await browserReload(browserId);
        setTimeout(() => { void browserSetLoadingState(browserId, false, null); }, 150);
    }

    async function captureScreenshot() {
        await browserCaptureScreenshot(browserId);
    }

    onMount(() => {
        if (browser?.current_url) address = browser.current_url;

        let unlisten: (() => void) | null = null;
        void listen<{ request_id: string; browser_id: string; action: BrowserAutomationAction }>('browser-automation-request', async (event) => {
            if (event.payload.browser_id !== browserId) return;
            try {
                const result = performAutomation(event.payload.action);
                result.request_id = event.payload.request_id;
                await browserAutomationComplete(event.payload.request_id, result);
            } catch (error) {
                await browserAutomationComplete(event.payload.request_id, null, error instanceof Error ? error.message : String(error));
            }
        }).then((d) => { unlisten = d; });

        return () => { unlisten?.(); };
    });
</script>

<section class="browser-shell">
    <header class="browser-toolbar">
        <!-- Nav buttons -->
        <div class="nav-buttons">
            <button class="nav-btn" type="button" onclick={goBack} title="Back" aria-label="Go back">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M7.5 2L3.5 6l4 4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
            <button class="nav-btn" type="button" onclick={goForward} title="Forward" aria-label="Go forward">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M4.5 2L8.5 6l-4 4" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
            <button class="nav-btn" type="button" onclick={reload} title="Reload" aria-label="Reload page">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M10 6A4 4 0 1 1 8 2.5L10 2v3h-3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
        </div>

        <!-- Address bar -->
        <form
            class="address-form"
            onsubmit={(e) => { e.preventDefault(); void navigate(); }}
        >
            <div class="address-bar" class:loading={browser?.is_loading}>
                {#if browser?.is_loading}
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
                />
            </div>
        </form>

        <!-- Utility buttons -->
        <div class="util-buttons">
            <button class="nav-btn" type="button" onclick={captureScreenshot} title="Capture screenshot" aria-label="Capture screenshot">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <rect x="1" y="2.5" width="10" height="7.5" rx="1.5" stroke="currentColor" stroke-width="1.2"/>
                    <circle cx="6" cy="6.5" r="1.8" stroke="currentColor" stroke-width="1.2"/>
                    <path d="M4 2.5l.5-1h3l.5 1" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
                </svg>
            </button>
            <button class="nav-btn" type="button" onclick={openExternal} title="Open in external browser" aria-label="Open externally">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M5 2H2.5A1.5 1.5 0 0 0 1 3.5v6A1.5 1.5 0 0 0 2.5 11h6A1.5 1.5 0 0 0 10 9.5V7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>
                    <path d="M7 1h4v4M11 1L6 6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
            </button>
        </div>
    </header>

    <div class="browser-body">
        {#if browser?.last_error}
            <div class="browser-overlay error">
                <h3>Failed to load</h3>
                <p>{browser.last_error}</p>
            </div>
        {:else}
            <iframe
                bind:this={iframeElement}
                title={browser?.title ?? 'Browser'}
                src={browser?.current_url ?? 'https://example.com'}
                class:loading={browser?.is_loading}
                onload={() => browserSetLoadingState(browserId, false, null)}
                onerror={() => browserSetLoadingState(browserId, false, 'Failed to load page')}
            ></iframe>
        {/if}

        {#if browser?.last_screenshot_path}
            <div class="status-toast">
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M2 6l3 3 5-5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                Saved: {browser.last_screenshot_path}
            </div>
        {/if}

        {#if automationResult}
            <pre class="automation-result">{automationResult}</pre>
        {/if}

        <div class="browser-hint">
            <p>Use <strong>Open</strong> to launch the URL externally until embedded rendering is available.</p>
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

    /* ---- Toolbar ---- */

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
        border-radius: 5px;
        color: var(--ui-text-muted);
        cursor: pointer;
        font: inherit;
        transition:
            background var(--ui-motion-fast),
            color var(--ui-motion-fast),
            border-color var(--ui-motion-fast);
        padding: 0;
    }

    .nav-btn:hover {
        background: var(--ui-layer-2);
        color: var(--ui-text-primary);
        border-color: var(--ui-border-soft);
    }

    /* ---- Address bar ---- */

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
        border-radius: 5px;
        transition:
            border-color var(--ui-motion-fast),
            background var(--ui-motion-fast);
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
        font: inherit;
        font-size: 0.78rem;
        padding: 0;
        outline: none;
    }

    .address-input::placeholder {
        color: var(--ui-text-muted);
    }

    /* ---- Browser body ---- */

    .browser-body {
        position: relative;
        flex: 1;
        width: 100%;
        min-height: 0;
        overflow: hidden;
    }

    iframe {
        display: block;
        width: 100%;
        height: 100%;
        border: 0;
        background: white;
        transition: opacity var(--ui-motion-fast);
    }

    iframe.loading {
        opacity: 0.6;
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

    .status-toast {
        position: absolute;
        bottom: 12px;
        right: 12px;
        z-index: 10;
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        font-size: 0.74rem;
        color: var(--ui-success);
    }

    .automation-result {
        position: absolute;
        top: 12px;
        left: 12px;
        right: 12px;
        z-index: 10;
        max-height: 160px;
        overflow: auto;
        padding: 10px;
        background: var(--ui-layer-2);
        border: 1px solid var(--ui-border-soft);
        border-radius: 6px;
        font-size: 0.74rem;
        white-space: pre-wrap;
        color: var(--ui-text-primary);
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
        border-radius: 6px;
        opacity: 0.7;
    }

    .browser-hint p {
        margin: 0;
        font-size: 0.72rem;
        color: var(--ui-text-muted);
        line-height: 1.4;
    }

    .browser-hint strong {
        color: var(--ui-text-secondary);
    }
</style>
