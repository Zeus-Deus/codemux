import { listen } from '@tauri-apps/api/event';
import { browserService } from './browserService';
import { invoke } from '@tauri-apps/api/core';

export interface BrowserAutomationRequest {
    request_id: string;
    browser_id: string;
    action: BrowserAutomationAction;
}

export type BrowserAutomationAction =
    | { kind: 'open_url'; url: string }
    | { kind: 'dom_snapshot' }
    | { kind: 'accessibility_snapshot' }
    | { kind: 'click'; selector: string }
    | { kind: 'fill'; selector: string; value: string }
    | { kind: 'type_text'; text: string }
    | { kind: 'scroll'; x: number; y: number }
    | { kind: 'evaluate'; script: string }
    | { kind: 'screenshot' }
    | { kind: 'console_logs' };

export interface BrowserAutomationResult {
    request_id: string;
    browser_id: string;
    data: unknown;
    message?: string;
}

async function handleAutomationRequest(request: BrowserAutomationRequest): Promise<BrowserAutomationResult> {
    const { browser_id, request_id } = request;
    const browserId = browser_id;

    try {
        let data: unknown;
        let message: string | undefined;

        switch (request.action.kind) {
            case 'open_url': {
                await browserService.spawn(browserId);
                await browserService.navigate(browserId, request.action.url);
                data = { url: request.action.url };
                message = `Navigated to ${request.action.url}`;
                break;
            }
            case 'dom_snapshot': {
                const result = await browserService.snapshot(browserId, { interactive: false, compact: false });
                data = { tree: result.tree };
                break;
            }
            case 'accessibility_snapshot': {
                const result = await browserService.snapshot(browserId, { interactive: true, compact: false });
                data = { tree: result.tree, refs: result.refs };
                break;
            }
            case 'click': {
                await browserService.click(browserId, request.action.selector);
                data = { selector: request.action.selector };
                message = `Clicked ${request.action.selector}`;
                break;
            }
            case 'fill': {
                await browserService.fill(browserId, request.action.selector, request.action.value);
                data = { selector: request.action.selector, value: request.action.value };
                message = `Filled ${request.action.selector}`;
                break;
            }
            case 'type_text': {
                await browserService.type(browserId, '', request.action.text);
                data = { text: request.action.text };
                break;
            }
            case 'scroll': {
                const script = `window.scrollTo(${request.action.x}, ${request.action.y})`;
                await browserService.eval(browserId, script);
                data = { x: request.action.x, y: request.action.y };
                break;
            }
            case 'evaluate': {
                const result = await browserService.eval(browserId, request.action.script);
                data = { result };
                break;
            }
            case 'screenshot': {
                const screenshot = await browserService.screenshot(browserId);
                data = { screenshot };
                break;
            }
            case 'console_logs': {
                const logs = await browserService.getConsoleLogs(browserId);
                data = { logs };
                break;
            }
            default: {
                throw new Error(`Unknown action: ${JSON.stringify(request.action)}`);
            }
        }

        return { request_id, browser_id, data, message };
    } catch (error) {
        throw error;
    }
}

export async function initBrowserAutomation() {
    const unlisten = await listen<BrowserAutomationRequest>('browser-automation-request', async (event) => {
        const request = event.payload;
        console.log('[browser-automation] Received request:', request.request_id, request.action.kind);

        try {
            const result = await handleAutomationRequest(request);
            await invoke('browser_automation_complete', {
                requestId: request.request_id,
                result: { Ok: result }
            });
            console.log('[browser-automation] Completed request:', request.request_id);
        } catch (error) {
            const errorMessage = error instanceof Error ? error.message : String(error);
            console.error('[browser-automation] Error:', errorMessage);
            await invoke('browser_automation_complete', {
                requestId: request.request_id,
                result: { Err: errorMessage }
            });
        }
    });

    return unlisten;
}
