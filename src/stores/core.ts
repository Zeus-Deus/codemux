import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';
import type { AppStateSnapshot } from './types';

export const appState = writable<AppStateSnapshot | null>(null);

export async function syncAppState() {
    const snapshot = await invoke<AppStateSnapshot>('get_app_state');
    appState.set(snapshot);
    return snapshot;
}

export async function initAppState() {
    try {
        await syncAppState();
    } catch (error) {
        console.error('Failed to fetch app state:', error);
    }

    let lastStateJson = '';
    let debounceTimer: ReturnType<typeof setTimeout> | null = null;

    await listen<AppStateSnapshot>('app-state-changed', (event) => {
        if (debounceTimer) clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => {
            const json = JSON.stringify(event.payload);
            if (json === lastStateJson) return;
            lastStateJson = json;
            appState.set(event.payload);
        }, 16);
    });
}
