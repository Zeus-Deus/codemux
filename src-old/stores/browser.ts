import { invoke } from '@tauri-apps/api/core';

export async function browserOpenUrl(browserId: string, url: string) {
    return invoke('browser_open_url', { browserId, url });
}

export async function browserHistoryBack(browserId: string) {
    return invoke('browser_history_back', { browserId });
}

export async function browserHistoryForward(browserId: string) {
    return invoke('browser_history_forward', { browserId });
}

export async function browserReload(browserId: string) {
    return invoke('browser_reload', { browserId });
}

export async function browserSetLoadingState(browserId: string, isLoading: boolean, error?: string | null) {
    return invoke('browser_set_loading_state', { browserId, isLoading, error });
}

export async function browserSpawn(browserId: string) {
    return invoke<string>('browser_spawn', { browserId });
}

export async function browserNavigate(browserId: string, url: string) {
    return invoke<string>('browser_navigate', { browserId, url });
}

export async function browserScreenshot(browserId: string) {
    return invoke<string>('browser_screenshot', { browserId });
}

export async function browserClick(browserId: string, x: number, y: number) {
    return invoke<string>('browser_click', { browserId, x, y });
}

export async function browserType(browserId: string, text: string) {
    return invoke<string>('browser_type', { browserId, text });
}

export async function browserClose(browserId: string) {
    return invoke('browser_close', { browserId });
}

export async function browserResizeViewport(browserId: string, width: number, height: number) {
    return invoke('browser_resize_viewport', { browserId, width, height });
}

export async function agentBrowserSpawn(browserId: string) {
    return invoke('agent_browser_spawn', { browserId });
}

export async function agentBrowserRun(browserId: string, action: string, params: Record<string, unknown> = {}) {
    return invoke('agent_browser_run', { browserId, action, params });
}

export async function agentBrowserClose(browserId: string) {
    return invoke('agent_browser_close', { browserId });
}

export async function agentBrowserGetStreamUrl() {
    return invoke<string>('agent_browser_get_stream_url');
}

export async function agentBrowserScreenshot(browserId: string) {
    return invoke<string>('agent_browser_screenshot', { browserId });
}
