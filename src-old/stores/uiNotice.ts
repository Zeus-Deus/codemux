import { writable } from 'svelte/store';

export interface UiNotice {
    kind: 'info' | 'error';
    message: string;
}

export const uiNotice = writable<UiNotice | null>(null);

let clearTimer: ReturnType<typeof setTimeout> | null = null;

export function clearUiNotice() {
    if (clearTimer !== null) {
        clearTimeout(clearTimer);
        clearTimer = null;
    }
    uiNotice.set(null);
}

export function showUiNotice(message: string, kind: UiNotice['kind'] = 'info', durationMs = 4500) {
    clearUiNotice();
    uiNotice.set({ kind, message });

    if (durationMs > 0) {
        clearTimer = setTimeout(() => {
            uiNotice.set(null);
            clearTimer = null;
        }, durationMs);
    }
}

export function errorMessage(error: unknown) {
    if (error instanceof Error) {
        return error.message;
    }

    if (typeof error === 'string') {
        return error;
    }

    return String(error);
}
