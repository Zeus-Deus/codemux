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
    let blockNewlineInput: ((e: Event) => void) | null = null;
    // Tracks whether the running application has pushed the kitty keyboard protocol
    // (e.g. OpenCode/crossterm sends ESC [ > 1 u to enable enhanced key reporting).
    // When active, Shift+Enter is sent as the CSI u sequence instead of \r.
    let kittyProtocolLevel = 0;
    const ptyDecoder = new TextDecoder('utf-8', { fatal: false });
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
            background: css.getPropertyValue('--ui-layer-0').trim() || '#0d0f11',
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

    // Scan PTY output for kitty keyboard protocol sequences.
    //
    // Detection works in two steps:
    //
    // Step 1 — capability query (crossterm sends \x1b[?u\x1b[c together):
    //   crossterm races the kitty-flags response against the DA1 response.
    //   If the kitty-flags response (\x1b[?<n>u) arrives FIRST, it knows the
    //   terminal supports keyboard enhancement.  xterm.js never responds to
    //   \x1b[?u, so crossterm only ever sees the DA1 response and concludes
    //   "not supported."  We fix this by detecting \x1b[?u in PTY output and
    //   immediately writing \x1b[?0u back to the PTY (meaning "supported, but
    //   no flags currently active").  This invoke is dispatched *before*
    //   term.write() runs, so it beats xterm.js's own DA1 response through the
    //   onData → queueMicrotask → invoke chain.
    //
    // Step 2 — push/pop tracking:
    //   Once crossterm knows the terminal supports enhancement it pushes its
    //   desired flags (\x1b[>Nu).  We count push/pop depth so that
    //   customKeyEventHandler knows when to send CSI u sequences vs plain \r.
    function scanKittyProtocol(data: Uint8Array | string): void {
        const str = typeof data === 'string' ? data : ptyDecoder.decode(data);

        // Step 1: respond to capability query before DA1 response can arrive.
        // \x1b[?u with no digits = the kitty keyboard capability query.
        // (A DEC private sequence \x1b[?<digits>h/l would have digits before the
        // final byte, so \x1b[?u cannot be confused with those.)
        if (str.includes('\x1b[?u')) {
            invoke('write_to_pty', { data: '\x1b[?0u', sessionId }).catch(console.error);
        }

        // Step 2: track push/pop depth.
        // Push: ESC [ > <flags> u  (e.g. \x1b[>1u sent by crossterm after detecting support)
        // Pop:  ESC [ < u
        const pushes = (str.match(/\x1b\[>[0-9]+u/g) ?? []).length;
        const pops   = (str.match(/\x1b\[<u/g)       ?? []).length;
        kittyProtocolLevel = Math.max(0, kittyProtocolLevel + pushes - pops);
    }

    function writePtyChunk(payload: unknown) {
        if (!term) {
            return;
        }

        if (payload instanceof Uint8Array) {
            scanKittyProtocol(payload);
            term.write(payload);
            return;
        }

        if (payload instanceof ArrayBuffer) {
            const data = new Uint8Array(payload);
            scanKittyProtocol(data);
            term.write(data);
            return;
        }

        if (Array.isArray(payload)) {
            const data = new Uint8Array(payload as number[]);
            scanKittyProtocol(data);
            term.write(data);
            return;
        }

        if (typeof payload === 'string') {
            scanKittyProtocol(payload);
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

        // New session: clear any previously tracked keyboard protocol state.
        kittyProtocolLevel = 0;

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

        // Add custom key handler for special key combos.
        //
        // IMPORTANT: xterm.js calls customKeyEventHandler for keydown, keypress,
        // AND keyup events.  WebKit fires keypress for Enter, so without the
        // `ev.type !== 'keydown'` guard below, every Shift+Enter would invoke
        // this handler twice and send two sequences to the PTY.  We guard all
        // data-sending branches to only act on keydown; for keypress/keyup we
        // still return false to suppress xterm's own handling of the same combo.
        term.attachCustomKeyEventHandler((ev) => {
            // Shift+Enter
            // - Kitty protocol active (e.g. OpenCode): send CSI 13;2u so the app sees
            //   "Shift+Enter" and inserts a newline without submitting.
            // - Kitty protocol not active (regular shell): return true so xterm handles
            //   it normally, sending \r — identical to plain Enter, which is the correct
            //   behaviour in a standard terminal.
            if (ev.shiftKey && ev.key === 'Enter') {
                if (kittyProtocolLevel > 0) {
                    if (ev.type === 'keydown') {
                        invoke('write_to_pty', { data: '\x1b[13;2u', sessionId }).catch(console.error);
                    }
                    ev.preventDefault?.();
                    return false;
                }
                return true;
            }
            // Ctrl+Backspace -> send Ctrl+W (backward-kill-word)
            if (ev.ctrlKey && ev.key === 'Backspace') {
                if (ev.type === 'keydown') {
                    invoke('write_to_pty', { data: '\x17', sessionId }).catch(console.error);
                }
                ev.preventDefault?.();
                return false;
            }
            // Ctrl+Shift+C -> copy (when text is selected)
            if (ev.ctrlKey && ev.shiftKey && ev.key === 'C') {
                if (ev.type === 'keydown') {
                    const selection = term?.getSelection();
                    if (selection) {
                        navigator.clipboard.writeText(selection).catch(console.error);
                    }
                }
                ev.preventDefault?.();
                return false;
            }
            // Ctrl+Shift+V -> paste
            if (ev.ctrlKey && ev.shiftKey && ev.key === 'V') {
                if (ev.type === 'keydown') {
                    navigator.clipboard.readText().then((text) => {
                        if (text && term) {
                            term.paste(text);
                        }
                    }).catch(console.error);
                }
                ev.preventDefault?.();
                return false;
            }
            // Tab management shortcuts — let these bubble to the window handler
            // instead of being consumed by xterm. Return false without
            // preventDefault so App.svelte's handleWindowKeydown picks them up.
            if (ev.ctrlKey && !ev.altKey) {
                const key = ev.key.toLowerCase();
                // Ctrl+T (new terminal tab), Ctrl+W (close tab)
                if (!ev.shiftKey && (key === 't' || key === 'w')) return false;
                // Ctrl+1 through Ctrl+9 (switch tab by index)
                if (!ev.shiftKey && ev.key >= '1' && ev.key <= '9') return false;
                // Ctrl+Shift+B (new browser tab), Ctrl+Shift+D (new diff tab)
                if (ev.shiftKey && (key === 'b' || key === 'd')) return false;
                // Ctrl+] and Ctrl+[ (workspace cycling)
                if (ev.key === ']' || ev.key === '[') return false;
            }
            return true;
        });

        fitAddon = new FitAddon();
        term.loadAddon(fitAddon);
        
        const clipboardAddon = new ClipboardAddon();
        term.loadAddon(clipboardAddon);
        
        const searchAddon = new SearchAddon();
        term.loadAddon(searchAddon);
        
        term.open(terminalContainer);

        // WKWebView (Tauri) does not reliably suppress `input` events on the
        // xterm helper textarea even after `preventDefault()` on `keydown`.
        // When kitty protocol is active and we intercept Shift+Enter in
        // `customKeyEventHandler`, the WKWebView still fires an `input` event
        // with `inputType: "insertText"` and `data: "\n"` (or
        // `"insertLineBreak"` with `data: null`).  xterm's `_inputEvent`
        // handler would call `onData("\n")` for the `insertText` variant,
        // doubling the newline we already sent as `\x1b[13;2u`.
        //
        // Fix: attach a capture-phase `input` listener on the container so it
        // fires before xterm's textarea listener.  When kitty protocol is
        // active we cancel any `input` event that would insert a newline.
        blockNewlineInput = (e: Event) => {
            if (kittyProtocolLevel <= 0) return;
            const ie = e as InputEvent;
            if (
                ie.inputType === 'insertLineBreak' ||
                ie.inputType === 'insertParagraph' ||
                (ie.inputType === 'insertText' && ie.data === '\n')
            ) {
                // Stop the event from reaching the xterm textarea listener so
                // xterm's _inputEvent handler never calls onData.  Use
                // stopPropagation (not stopImmediatePropagation) because xterm's
                // listener is on the child <textarea>, not on this container.
                e.stopPropagation();
                e.preventDefault();
            }
        };
        terminalContainer.addEventListener('input', blockNewlineInput, true);

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
            if (blockNewlineInput) {
                terminalContainer.removeEventListener('input', blockNewlineInput, true);
                blockNewlineInput = null;
            }
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
        background: var(--ui-layer-0, #0d0f11);
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
        border-radius: var(--ui-radius-lg);
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
