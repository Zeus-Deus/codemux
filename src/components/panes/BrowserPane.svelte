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
        $appState?.browser_sessions.find((entry) => entry.browser_id === browserId) ?? null
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
        if (!doc) {
            throw new Error('Browser document is not available yet');
        }
        return doc;
    }

    function performAutomation(action: BrowserAutomationAction): BrowserAutomationResult {
        switch (action.kind) {
            case 'open_url':
                address = action.url;
                void navigate(action.url);
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { url: action.url },
                    message: 'Navigation requested'
                };
            case 'dom_snapshot': {
                const doc = ensureSameOriginDocument();
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: {
                        title: doc.title,
                        body_text: doc.body?.innerText?.slice(0, 4000) ?? ''
                    },
                    message: 'DOM snapshot captured'
                };
            }
            case 'accessibility_snapshot': {
                const doc = ensureSameOriginDocument();
                const elements = Array.from(doc.querySelectorAll('button, a, input, textarea, select, [role]'))
                    .slice(0, 100)
                    .map((element) => ({
                        tag: element.tagName.toLowerCase(),
                        role: element.getAttribute('role'),
                        text: element.textContent?.trim() ?? '',
                        aria_label: element.getAttribute('aria-label')
                    }));
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { elements },
                    message: 'Accessibility snapshot captured'
                };
            }
            case 'click': {
                const doc = ensureSameOriginDocument();
                const element = doc.querySelector<HTMLElement>(action.selector);
                if (!element) {
                    throw new Error(`No element matched selector ${action.selector}`);
                }
                element.click();
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { selector: action.selector },
                    message: 'Element clicked'
                };
            }
            case 'fill': {
                const doc = ensureSameOriginDocument();
                const element = doc.querySelector<HTMLInputElement | HTMLTextAreaElement>(action.selector);
                if (!element) {
                    throw new Error(`No fill target matched selector ${action.selector}`);
                }
                element.value = action.value;
                element.dispatchEvent(new Event('input', { bubbles: true }));
                element.dispatchEvent(new Event('change', { bubbles: true }));
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { selector: action.selector, value: action.value },
                    message: 'Element filled'
                };
            }
            case 'type_text': {
                const win = frameWindow();
                if (!win) {
                    throw new Error('Browser window is not available yet');
                }
                win.document.activeElement?.dispatchEvent(new KeyboardEvent('keydown', { key: action.text }));
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { text: action.text },
                    message: 'Text typing event dispatched'
                };
            }
            case 'scroll': {
                const win = frameWindow();
                if (!win) {
                    throw new Error('Browser window is not available yet');
                }
                win.scrollBy(action.x, action.y);
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { x: action.x, y: action.y },
                    message: 'Scroll dispatched'
                };
            }
            case 'evaluate': {
                const win = frameWindow();
                if (!win) {
                    throw new Error('Browser window is not available yet');
                }
                const value = (win as Window & typeof globalThis & { eval: (script: string) => unknown }).eval(action.script);
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { value },
                    message: 'Script evaluated'
                };
            }
            case 'screenshot': {
                void captureScreenshot();
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { requested: true },
                    message: 'Screenshot requested'
                };
            }
            case 'console_logs':
                return {
                    request_id: '',
                    browser_id: browserId,
                    data: { logs: automationLog },
                    message: 'Console log buffer returned'
                };
        }
    }

    async function navigate(url = address) {
        if (!url.trim()) {
            return;
        }

        await browserOpenUrl(browserId, url);
        setTimeout(() => {
            void browserSetLoadingState(browserId, false, null);
        }, 300);
    }

    async function openExternal() {
        if (!address.trim()) {
            return;
        }

        await openUrl(address.trim());
    }

    async function goBack() {
        await browserHistoryBack(browserId);
        setTimeout(() => {
            void browserSetLoadingState(browserId, false, null);
        }, 150);
    }

    async function goForward() {
        await browserHistoryForward(browserId);
        setTimeout(() => {
            void browserSetLoadingState(browserId, false, null);
        }, 150);
    }

    async function reload() {
        await browserReload(browserId);
        setTimeout(() => {
            void browserSetLoadingState(browserId, false, null);
        }, 150);
    }

    async function captureScreenshot() {
        await browserCaptureScreenshot(browserId);
    }

    async function runAutomation(action: BrowserAutomationAction) {
        const result = await browserAutomationRun(browserId, action);
        automationResult = JSON.stringify(result.data, null, 2);
    }

    onMount(() => {
        if (browser?.current_url) {
            address = browser.current_url;
        }

        let unlisten: (() => void) | null = null;
        void listen<{
            request_id: string;
            browser_id: string;
            action: BrowserAutomationAction;
        }>('browser-automation-request', async (event) => {
            if (event.payload.browser_id !== browserId) {
                return;
            }

            try {
                const result = performAutomation(event.payload.action);
                result.request_id = event.payload.request_id;
                await browserAutomationComplete(event.payload.request_id, result);
            } catch (error) {
                await browserAutomationComplete(
                    event.payload.request_id,
                    null,
                    error instanceof Error ? error.message : String(error)
                );
            }
        }).then((dispose) => {
            unlisten = dispose;
        });

        return () => {
            unlisten?.();
        };
    });
</script>

<section class="browser-shell">
    <header class="browser-toolbar">
        <div class="browser-actions">
            <button type="button" onclick={goBack}>&larr;</button>
            <button type="button" onclick={goForward}>&rarr;</button>
            <button type="button" onclick={reload}>Reload</button>
            <button type="button" onclick={captureScreenshot}>Shot</button>
            <button
                type="button"
                onclick={() => runAutomation({ kind: 'dom_snapshot' })}
            >DOM</button>
            <button type="button" onclick={openExternal}>Open</button>
        </div>
        <form
            class="address-form"
            onsubmit={(event) => {
                event.preventDefault();
                void navigate();
            }}
        >
            <input bind:value={address} placeholder="Enter URL" />
        </form>
    </header>

    <div class="browser-body">
        {#if browser?.last_error}
            <div class="browser-state error">
                <h3>Browser error</h3>
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

        {#if browser?.is_loading}
            <div class="loading-banner">Loading {browser.current_url}...</div>
        {/if}

        {#if browser?.last_screenshot_path}
            <div class="screenshot-banner">Saved screenshot: {browser.last_screenshot_path}</div>
        {/if}

        {#if automationResult}
            <pre class="automation-result">{automationResult}</pre>
        {/if}

        <div class="browser-state info-inline">
            <h3>Browser MVP</h3>
            <p>Use `Open` to launch the current URL externally until embedded rendering is finished.</p>
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
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 98%, black 2%);
    }

    .browser-toolbar {
        display: flex;
        gap: 10px;
        padding: 8px 10px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 92%, transparent);
        border-bottom: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 8%, transparent);
        flex: 0 0 auto;
    }

    .browser-actions {
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
    }

    .browser-actions button,
    .address-form input {
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 12%, transparent);
        border-radius: 8px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 82%, transparent);
        color: inherit;
        padding: 6px 8px;
        transition:
            border-color 100ms ease-out,
            background 100ms ease-out,
            color 100ms ease-out;
    }

    .browser-actions button:hover {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 24%, transparent);
        background: color-mix(in srgb, var(--theme-accent, #7aa2f7) 10%, transparent);
    }

    .address-form {
        flex: 1;
    }

    .address-form input {
        width: 100%;
        box-sizing: border-box;
        outline: none;
    }

    .address-form input:focus {
        border-color: color-mix(in srgb, var(--theme-accent, #7aa2f7) 26%, transparent);
    }

    .browser-body {
        position: relative;
        flex: 1;
        width: 100%;
        min-height: 0;
        overflow: hidden;
        padding: 8px;
        box-sizing: border-box;
    }

    iframe {
        display: block;
        width: 100%;
        height: 100%;
        border: 0;
        border-radius: 10px;
        background: white;
    }

    iframe.loading {
        opacity: 0.72;
    }

    .loading-banner,
    .screenshot-banner,
    .browser-state,
    .automation-result {
        position: absolute;
        left: 16px;
        right: 16px;
        z-index: 2;
        border-radius: 10px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 90%, white 10%);
        border: 1px solid color-mix(in srgb, var(--theme-accent, #7aa2f7) 22%, transparent);
    }

    .loading-banner {
        bottom: 16px;
        padding: 10px 12px;
    }

    .screenshot-banner {
        bottom: 60px;
        padding: 10px 12px;
    }

    .automation-result {
        top: 16px;
        max-height: 160px;
        overflow: auto;
        padding: 12px;
        white-space: pre-wrap;
    }

    .browser-state {
        top: 16px;
        padding: 12px;
    }

    .browser-state.info-inline {
        top: auto;
        bottom: 16px;
        right: auto;
        max-width: 360px;
        opacity: 0.92;
    }

    .browser-state h3,
    .browser-state p {
        margin: 0;
    }

    .browser-state p {
        margin-top: 6px;
    }
</style>
