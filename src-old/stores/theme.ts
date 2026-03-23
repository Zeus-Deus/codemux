import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export interface ThemeColors {
    accent: string;
    cursor: string;
    foreground: string;
    background: string;
    selection_foreground: string;
    selection_background: string;
    color0: string;
    color1: string;
    color2: string;
    color3: string;
    color4: string;
    color5: string;
    color6: string;
    color7: string;
    color8: string;
    color9: string;
    color10: string;
    color11: string;
    color12: string;
    color13: string;
    color14: string;
    color15: string;
}

export interface ShellAppearance {
    font_family: string;
}

export const theme = writable<ThemeColors | null>(null);
export const shellAppearance = writable<ShellAppearance | null>(null);

export const fallbackTheme: ThemeColors = {
    accent: '#7aa2f7',
    cursor: '#c0caf5',
    foreground: '#c0caf5',
    background: '#1a1b26',
    selection_foreground: '#c0caf5',
    selection_background: '#283457',
    color0: '#15161e',
    color1: '#f7768e',
    color2: '#9ece6a',
    color3: '#e0af68',
    color4: '#7aa2f7',
    color5: '#bb9af7',
    color6: '#7dcfff',
    color7: '#a9b1d6',
    color8: '#414868',
    color9: '#f7768e',
    color10: '#9ece6a',
    color11: '#e0af68',
    color12: '#7aa2f7',
    color13: '#bb9af7',
    color14: '#7dcfff',
    color15: '#c0caf5'
};

export async function initTheme() {
    try {
        const initialTheme = await invoke<ThemeColors>('get_current_theme');
        theme.set(initialTheme);
    } catch (e) {
        console.error('Failed to get initial theme:', e);
        theme.set(fallbackTheme);
    }

    await listen<ThemeColors>('theme-changed', (event) => {
        theme.set(event.payload);
    });

    try {
        const appearance = await invoke<ShellAppearance>('get_shell_appearance');
        shellAppearance.set(appearance);
    } catch (error) {
        console.error('Failed to get shell appearance:', error);
        shellAppearance.set({ font_family: 'monospace' });
    }
}
