<script lang="ts">
    import { onMount } from 'svelte';
    import { Terminal } from '@xterm/xterm';
    import type { ITheme } from '@xterm/xterm';
    import { FitAddon } from '@xterm/addon-fit';
    import { WebglAddon } from '@xterm/addon-webgl';
    import { ClipboardAddon } from '@xterm/addon-clipboard';
    import { SearchAddon } from '@xterm/addon-search';
    import { Channel, invoke } from '@tauri-apps/api/core';
    import { listen } from '@tauri-apps/api/event';
    import { theme, fallbackTheme, shellAppearance, type ShellAppearance } from '../../stores/theme';
    import '@xterm/xterm/css/xterm.css';

    type DisposeHandle = { dispose: () => void };

    let { sessionId }: { sessionId: string } = $props();

    interface TerminalStatusPayload {
        session_id: string;
        state: 'starting' | 'ready' | 'exited' | 'failed';
        message: string | null;
        exit_code: number | null;
    }

    let terminalContainer: HTMLDivElement;
    let term: Terminal | null = null;
    let fitAddon: FitAddon | null = null;
    let resizeObserver: ResizeObserver | null = null;
    let resizeDebounceTimer: ReturnType<typeof setTimeout> | null = null;
    let dataDisposable: DisposeHandle | null = null;
    let resizeHandler: (() => void) | null = null;
    let themeUnsubscribe: (() => void) | null = null;
    let shellAppearanceUnsubscribe: (() => void) | null = null;
    let statusUnlisten: (() => void) | null = null;
    let attachedSessionId: string | null = null;
    let terminalStatus = $state<TerminalStatusPayload>({
        session_id: '',
        state: 'starting',
        message: 'Starting shell...',
        exit_code: null
    });

    $effect(() => {
        terminalStatus.session_id = sessionId;
    });

    function terminalTheme(): ITheme {
        const css = getComputedStyle(document.documentElement);
        return {
            background: css.getPropertyValue('--theme-background').trim() || fallbackTheme.background,
            foreground: css.getPropertyValue('--theme-foreground').trim() || fallbackTheme.foreground,
            cursor: css.getPropertyValue('--theme-cursor').trim() || fallbackTheme.cursor,
            selectionBackground:
                css.getPropertyValue('--theme-selection-background').trim() || fallbackTheme.selection_background,
            selectionForeground:
                css.getPropertyValue('--theme-selection-foreground').trim() || fallbackTheme.selection_foreground,
            black: css.getPropertyValue('--theme-color0').trim() || fallbackTheme.color0,
            red: css.getPropertyValue('--theme-color1').trim() || fallbackTheme.color1,
            green: css.getPropertyValue('--theme-color2').trim() || fallbackTheme.color2,
            yellow: css.getPropertyValue('--theme-color3').trim() || fallbackTheme.color3,
            blue: css.getPropertyValue('--theme-color4').trim() || fallbackTheme.color4,
            magenta: css.getPropertyValue('--theme-color5').trim() || fallbackTheme.color5,
            cyan: css.getPropertyValue('--theme-color6').trim() || fallbackTheme.color6,
            white: css.getPropertyValue('--theme-color7').trim() || fallbackTheme.color7,
            brightBlack: css.getPropertyValue('--theme-color8').trim() || fallbackTheme.color8,
            brightRed: css.getPropertyValue('--theme-color9').trim() || fallbackTheme.color9,
            brightGreen: css.getPropertyValue('--theme-color10').trim() || fallbackTheme.color10,
            brightYellow: css.getPropertyValue('--theme-color11').trim() || fallbackTheme.color11,
            brightBlue: css.getPropertyValue('--theme-color12').trim() || fallbackTheme.color12,
            brightMagenta: css.getPropertyValue('--theme-color13').trim() || fallbackTheme.color13,
            brightCyan: css.getPropertyValue('--theme-color14').trim() || fallbackTheme.color14,
            brightWhite: css.getPropertyValue('--theme-color15').trim() || fallbackTheme.color15
        };
    }

    function applyTerminalTheme() {
        if (!term) {
            return;
        }

        term.options.theme = terminalTheme();
    }

    function writePtyChunk(payload: unknown) {
        if (!term) {
            return;
        }

        if (payload instanceof Uint8Array) {
            term.write(payload);
            return;
        }

        if (payload instanceof ArrayBuffer) {
            term.write(new Uint8Array(payload));
            return;
        }

        if (Array.isArray(payload)) {
            term.write(new Uint8Array(payload as number[]));
            return;
        }

        if (typeof payload === 'string') {
            term.write(payload);
        }
    }

    async function syncTerminalSize() {
        if (!term || !fitAddon) {
            return;
        }

        fitAddon.fit();

        if (term.cols === 0 || term.rows === 0) {
            return;
        }

        try {
            await invoke('resize_pty', { cols: term.cols, rows: term.rows, sessionId: sessionId });
        } catch (error) {
            console.error(`Failed to resize PTY for ${sessionId}:`, error);
        }
    }

    async function attachSession() {
        if (!term) {
            return;
        }

        if (attachedSessionId === sessionId) {
            await syncTerminalSize();
            return;
        }

        // Remember whether we're re-attaching to the *same* session that was
        // previously wired up (e.g. after a pane resize). In that case xterm
        // already holds the correct visible content, so we can skip clearing
        // the buffer and replaying the full pending_output history.
        // If we're switching to a *different* session (attachedSessionId !== null
        // but !== sessionId) we must clear so old content doesn't bleed
        // through — the early-return above already handles the identical-session
        // fast path, so reaching this point always means a session switch.
        const isReattachingSameSession = false;

        if (attachedSessionId) {
            try {
                await invoke('detach_pty_output', { sessionId: attachedSessionId });
            } catch (error) {
                console.error(`Failed to detach terminal output for ${attachedSessionId}:`, error);
            }
        }

        if (!isReattachingSameSession) {
            term.clear();
        }
        attachedSessionId = sessionId;

        try {
            terminalStatus = await invoke<TerminalStatusPayload>('get_terminal_status', { sessionId: sessionId });
        } catch (error) {
            terminalStatus = {
                session_id: sessionId,
                state: 'failed',
                message: `Failed to read terminal status: ${String(error)}`,
                exit_code: null
            };
        }

        const ptyOutChannel = new Channel<unknown>((payload) => {
            writePtyChunk(payload);
        });

        try {
            await invoke('attach_pty_output', {
                channel: ptyOutChannel,
                sessionId: sessionId,
                skipPending: isReattachingSameSession,
            });
        } catch (error) {
            terminalStatus = {
                session_id: sessionId,
                state: 'failed',
                message: `Failed to attach terminal output: ${String(error)}`,
                exit_code: null
            };
        }

        await syncTerminalSize();
    }

    onMount(async () => {
        term = new Terminal({
            fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--shell-font-family').trim() || 'monospace',
            theme: terminalTheme(),
            allowProposedApi: false,
            convertEol: true,
            cursorBlink: true,
            cursorWidth: 2,
            lineHeight: 1.15,
            letterSpacing: 0,
            fontSize: 13,
            cursorStyle: 'bar',
            altClickMovesCursor: true
        });

        // Add custom key handler for Ctrl+Backspace -> Ctrl+W (delete word)
        term.attachCustomKeyEventHandler((ev) => {
            // Ctrl+Backspace -> send Ctrl+W (backward-kill-word)
            if (ev.ctrlKey && ev.key === 'Backspace') {
                invoke('write_to_pty', { data: '\x17', sessionId }).catch(console.error);
                ev.preventDefault?.();
                return false;
            }
            // Ctrl+Shift+C -> copy (when text is selected)
            if (ev.ctrlKey && ev.shiftKey && ev.key === 'C') {
                const selection = term?.getSelection();
                if (selection) {
                    navigator.clipboard.writeText(selection).catch(console.error);
                }
                ev.preventDefault?.();
                return false;
            }
            // Ctrl+Shift+V -> paste
            if (ev.ctrlKey && ev.shiftKey && ev.key === 'V') {
                navigator.clipboard.readText().then((text) => {
                    if (text && term) {
                        term.paste(text);
                    }
                }).catch(console.error);
                ev.preventDefault?.();
                return false;
            }
            return true;
        });

        // Handle Shift+Enter via DOM event to send newline (for OpenCode etc)
        const terminalElement = terminalContainer;
        if (terminalElement) {
            terminalElement.addEventListener('keydown', function handleShiftEnter(e: KeyboardEvent) {
                if (e.shiftKey && e.key === 'Enter') {
                    e.preventDefault();
                    e.stopPropagation();
                    invoke('write_to_pty', { data: '\n', sessionId }).catch(console.error);
                }
            });
        }

        fitAddon = new FitAddon();
        term.loadAddon(fitAddon);
        
        const clipboardAddon = new ClipboardAddon();
        term.loadAddon(clipboardAddon);
        
        const searchAddon = new SearchAddon();
        term.loadAddon(searchAddon);
        
        term.open(terminalContainer);

        try {
            const webglAddon = new WebglAddon();
            term.loadAddon(webglAddon);
        } catch (error) {
            console.warn('WebGL addon could not be loaded, falling back to canvas/dom renderer', error);
        }

        themeUnsubscribe = theme.subscribe(() => {
            applyTerminalTheme();
        });

        shellAppearanceUnsubscribe = shellAppearance.subscribe((appearance: ShellAppearance | null) => {
            if (!term) {
                return;
            }

            term.options.fontFamily = appearance?.font_family?.trim() || 'monospace';
            fitAddon?.fit();
        });

        statusUnlisten = await listen<TerminalStatusPayload>('terminal-status', (event) => {
            if (event.payload.session_id !== sessionId) {
                return;
            }
            terminalStatus = event.payload;
        });

        let pendingInput = '';
        let inputQueued = false;

        dataDisposable = term.onData((data) => {
            pendingInput += data;
            if (!inputQueued) {
                inputQueued = true;
                queueMicrotask(() => {
                    const batch = pendingInput;
                    pendingInput = '';
                    inputQueued = false;
                    invoke('write_to_pty', { data: batch, sessionId }).catch((error) => {
                        console.error(`Failed to write to PTY for ${sessionId}:`, error);
                        terminalStatus = {
                            session_id: sessionId,
                            state: 'failed',
                            message: `Failed to write to shell: ${String(error)}`,
                            exit_code: null
                        };
                    });
                });
            }
        });

        await attachSession();

        resizeHandler = () => {
            void syncTerminalSize();
        };
        window.addEventListener('resize', resizeHandler);

        resizeObserver = new ResizeObserver(() => {
            if (resizeDebounceTimer !== null) {
                clearTimeout(resizeDebounceTimer);
            }
            resizeDebounceTimer = setTimeout(() => {
                resizeDebounceTimer = null;
                void syncTerminalSize();
            }, 150);
        });
        resizeObserver.observe(terminalContainer);
    });

    $effect(() => {
        const sid = sessionId;
        if (!term || !sid || sid === attachedSessionId) {
            return;
        }

        void attachSession();
    });

    onMount(() => {
        return () => {
            if (attachedSessionId) {
                void invoke('detach_pty_output', { sessionId: attachedSessionId }).catch((error) => {
                    console.error(`Failed to detach terminal output for ${attachedSessionId}:`, error);
                });
            }
            if (resizeHandler) {
                window.removeEventListener('resize', resizeHandler);
            }
            if (dataDisposable) {
                dataDisposable.dispose();
            }
            if (statusUnlisten) {
                statusUnlisten();
            }
            if (themeUnsubscribe) {
                themeUnsubscribe();
            }
            if (shellAppearanceUnsubscribe) {
                shellAppearanceUnsubscribe();
            }
            if (resizeDebounceTimer !== null) {
                clearTimeout(resizeDebounceTimer);
                resizeDebounceTimer = null;
            }
            term?.dispose();
            resizeObserver?.disconnect();
        };
    });
</script>

<div class="terminal-shell">
    <div class="terminal-wrapper" bind:this={terminalContainer}></div>

    {#if terminalStatus.state !== 'ready'}
        <div class={`terminal-overlay ${terminalStatus.state}`}>
            <div class="overlay-card">
                <h2>{terminalStatus.state === 'failed' ? 'Terminal unavailable' : 'Terminal starting'}</h2>
                <p>{terminalStatus.message ?? 'Waiting for shell status...'}</p>
                {#if terminalStatus.exit_code !== null}
                    <span class="status-meta">Exit code: {terminalStatus.exit_code}</span>
                {/if}
            </div>
        </div>
    {/if}
</div>

<style>
    .terminal-shell {
        position: relative;
        display: flex;
        flex: 1;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 98%, black 2%);
    }

    .terminal-wrapper {
        display: block;
        flex: 1;
        width: 100%;
        height: 100%;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
        padding: 6px 8px 8px;
        box-sizing: border-box;
    }

    .terminal-overlay {
        position: absolute;
        inset: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        padding: 18px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 88%, black 12%);
    }

    .overlay-card {
        width: min(440px, 100%);
        padding: 16px;
        border: 1px solid color-mix(in srgb, var(--theme-foreground, #c0caf5) 12%, transparent);
        border-radius: 10px;
        background: color-mix(in srgb, var(--theme-background, #1a1b26) 90%, white 10%);
    }

    .overlay-card h2 {
        margin: 0 0 8px;
        font-size: 0.92rem;
        font-weight: 600;
        color: var(--theme-foreground, #c0caf5);
    }

    .overlay-card p {
        margin: 0;
        line-height: 1.45;
        color: color-mix(in srgb, var(--theme-foreground, #c0caf5) 78%, white 22%);
    }

    .status-meta {
        display: inline-block;
        margin-top: 12px;
        font-size: 0.74rem;
        color: var(--theme-accent, #7aa2f7);
    }

    .terminal-overlay.failed .overlay-card {
        border-color: color-mix(in srgb, #f7768e 45%, transparent);
    }

    :global(.terminal-wrapper .terminal) {
        height: 100%;
        width: 100%;
    }

    :global(.terminal-wrapper .xterm) {
        padding-top: 2px;
    }

    :global(.terminal-wrapper .xterm),
    :global(.terminal-wrapper .xterm-viewport),
    :global(.terminal-wrapper .xterm-screen),
    :global(.terminal-wrapper .xterm-helpers) {
        width: 100%;
        height: 100%;
    }

    :global(.terminal-wrapper .xterm-viewport) {
        background: transparent !important;
    }
</style>
