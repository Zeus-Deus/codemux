import { useEffect, useRef, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebglAddon } from "@xterm/addon-webgl";
import { isAppShortcut } from "@/lib/app-shortcuts";
import { matchesKeyCombo } from "@/lib/keybind-utils";
import {
  KITTY_FUNCTIONAL_KEYS,
  BACKSPACE_CODEPOINT,
  csiUModifier,
  csiUSequence,
  scanKittySequences,
  applyKittyStack,
  kittyFlags,
} from "@/lib/kitty-keyboard";
import { resolveKeybinds } from "@/hooks/use-resolved-keybinds";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import {
  getTerminalFontSize,
  getTerminalCursorStyle,
  getTerminalFontFamily,
} from "@/stores/settings-store";
import {
  writeToPty,
  resizePty,
  detachPtyOutput,
  attachPtyOutput,
  pausePtyOutput,
  resumePtyOutput,
  getTerminalStatus,
  clearAgentStatus,
  getTerminalScrollback,
  cacheTerminalScrollback,
  uncacheTerminalScrollback,
  Channel,
  type ScrollbackPayload,
} from "@/tauri/commands";
import { registerTerminalForSerialize } from "@/hooks/use-scrollback-serializer";
import { createWritePump } from "./terminal-write-pump";
import { useAppStore, getSessionWorkspaceId } from "@/stores/app-store";
import { onTerminalStatus } from "@/tauri/events";
// TODO: re-enable as "system theme" option in settings
// import { useThemeColors } from "@/hooks/use-theme-colors";
import { useTauriEvent } from "@/hooks/use-tauri-event";
import type { TerminalStatusPayload } from "@/tauri/types";

interface Props {
  sessionId: string;
  paneId?: string;
  focused: boolean;
  visible: boolean;
  title: string;
}

// Static ANSI palette — Ember warm-tinted colors
const ANSI_COLORS = {
  black: "#151110",
  red: "#dc6b6b",
  green: "#7ec699",
  yellow: "#e5c07b",
  blue: "#61afef",
  magenta: "#c678dd",
  cyan: "#56b6c2",
  white: "#eae8e6",
  brightBlack: "#5c5856",
  brightRed: "#e88888",
  brightGreen: "#98d1a8",
  brightYellow: "#ecd08f",
  brightBlue: "#7ec0f5",
  brightMagenta: "#d494e6",
  brightCyan: "#73c7d3",
  brightWhite: "#ffffff",
};

function resolveOklch(value: string): string {
  const el = document.createElement("div");
  el.style.color = value;
  document.body.appendChild(el);
  const rgb = getComputedStyle(el).color;
  document.body.removeChild(el);
  return rgb;
}

function getCSSVar(name: string): string {
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  if (!raw) return "";
  return resolveOklch(raw);
}

function buildThemeFromCSS(): ITheme {
  return {
    background: getCSSVar("--background"),
    foreground: getCSSVar("--foreground"),
    cursor: getCSSVar("--sidebar-primary"),
    cursorAccent: getCSSVar("--background"),
    selectionBackground: getCSSVar("--accent"),
    selectionForeground: getCSSVar("--accent-foreground"),
    ...ANSI_COLORS,
  };
}

// TODO: re-enable as "system theme" option in settings
// function buildXtermTheme(t: ThemeColors): ITheme {
//   return {
//     background: t.background, foreground: t.foreground, cursor: t.cursor,
//     selectionBackground: t.selection_background, selectionForeground: t.selection_foreground,
//     black: t.color0, red: t.color1, green: t.color2, yellow: t.color3,
//     blue: t.color4, magenta: t.color5, cyan: t.color6, white: t.color7,
//     brightBlack: t.color8, brightRed: t.color9, brightGreen: t.color10, brightYellow: t.color11,
//     brightBlue: t.color12, brightMagenta: t.color13, brightCyan: t.color14, brightWhite: t.color15,
//   };
// }

function extractBytes(payload: unknown): Uint8Array | null {
  if (payload instanceof Uint8Array) return payload;
  if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
  if (Array.isArray(payload)) return new Uint8Array(payload as number[]);
  if (typeof payload === "string") return new TextEncoder().encode(payload);
  return null;
}

export function TerminalPane({ sessionId, paneId, focused, visible }: Props) {
  // TODO: re-enable as "system theme" option in settings
  // const { theme, shellAppearance } = useThemeColors();

  // Refs for mutable state that persists across renders
  const shellRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const serializeAddonRef = useRef<SerializeAddon | null>(null);
  const webglAddonRef = useRef<WebglAddon | null>(null);
  const attachedSessionRef = useRef<string | null>(null);
  // Adapter captures from the scrollback restore — persists across tab switches
  // even when the pane state (layout.json) doesn't have them.
  const restoredCapturesRef = useRef<Record<string, string>>({});
  const kittyStackRef = useRef<number[]>([]);
  const kittyLevelRef = useRef(0);
  const statusRef = useRef<TerminalStatusPayload>({
    session_id: sessionId,
    state: "starting",
    message: "Starting shell...",
    exit_code: null,
  });
  const statusOverlayRef = useRef<HTMLDivElement>(null);
  const ptyDecoderRef = useRef(new TextDecoder("utf-8", { fatal: false }));
  const blockNewlineRef = useRef<((e: Event) => void) | null>(null);
  const dataDisposableRef = useRef<{ dispose: () => void } | null>(null);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const windowResizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Track latest sessionId for closures
  const sessionIdRef = useRef(sessionId);
  sessionIdRef.current = sessionId;
  // Track props for closures
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  // ── Kitty protocol scanning ──
  const scanKittyProtocol = useCallback((data: Uint8Array | string) => {
    const str =
      typeof data === "string"
        ? data
        : ptyDecoderRef.current.decode(data);

    const scan = scanKittySequences(str);

    // Respond to Kitty keyboard protocol query (\x1b[?u) with current flags.
    // Apps (Claude Code, nvim, etc.) send this to check if the terminal
    // supports enhanced key reporting before pushing Kitty mode.
    if (scan.hasQuery) {
      writeToPty(
        sessionIdRef.current,
        `\x1b[?${kittyFlags(kittyStackRef.current)}u`,
      ).catch(console.error);
    }

    // Apply push/pop/reset. DA query from a new shell resets stale state.
    kittyStackRef.current = applyKittyStack(
      kittyStackRef.current,
      scan.pushValues,
      scan.popCount,
      scan.hasDAQuery,
    );
    kittyLevelRef.current = kittyFlags(kittyStackRef.current);
  }, []);

  // ── Resize sync ──
  const syncTerminalSize = useCallback(async () => {
    const term = termRef.current;
    const fitAddon = fitAddonRef.current;
    if (!term || !fitAddon || !visibleRef.current) return;

    fitAddon.fit();
    if (term.cols === 0 || term.rows === 0) return;

    try {
      await resizePty(sessionIdRef.current, term.cols, term.rows);
    } catch (err) {
      console.error("Failed to resize PTY:", err);
    }
  }, []);

  // ── Update status overlay ──
  //
  // Mutates DOM directly (h2/p/.status-meta) rather than going
  // through React for perf — terminal status fires per IPC tick
  // and we don't want to schedule a re-render of the whole pane
  // for every status update.
  //
  // The visual state (spinner vs warning indicator) is also
  // toggled via display=flex/none on the two icon slots inside
  // .status-indicator — same DOM-mutation pattern. The Tailwind
  // classes on the slots define the static look; we just toggle
  // visibility based on state.
  const updateStatusOverlay = useCallback((status: TerminalStatusPayload) => {
    statusRef.current = status;
    const el = statusOverlayRef.current;
    if (!el) return;
    if (status.state === "ready") {
      el.style.display = "none";
      return;
    }
    el.style.display = "flex";
    // Keep the base classes (positioning, backdrop) and append
    // the state for any state-specific CSS hooks downstream.
    el.className = `terminal-overlay ${status.state} absolute inset-0 z-0 flex items-center justify-center p-6 bg-background/95 backdrop-blur-sm`;
    const failed = status.state === "failed";
    // Swap spinner vs warning indicator visibility.
    const spinner = el.querySelector<HTMLElement>(".status-indicator .spinner");
    const warning = el.querySelector<HTMLElement>(".status-indicator .warning");
    if (spinner) spinner.style.display = failed ? "none" : "block";
    if (warning) warning.style.display = failed ? "flex" : "none";
    const h2 = el.querySelector("h2");
    const p = el.querySelector("p");
    const code = el.querySelector(".status-meta");
    if (h2)
      h2.textContent = failed ? "Terminal unavailable" : "Terminal starting";
    if (p) p.textContent = status.message ?? "Waiting for shell status...";
    if (code)
      code.textContent =
        status.exit_code !== null ? `Exit code: ${status.exit_code}` : "";
  }, []);

  // ── Terminal status event ──
  useTauriEvent(
    onTerminalStatus,
    useCallback(
      (payload: TerminalStatusPayload) => {
        if (payload.session_id !== sessionIdRef.current) return;
        updateStatusOverlay(payload);
      },
      [updateStatusOverlay],
    ),
    [],
  );

  // ── Main mount/teardown effect ──
  useEffect(() => {
    const sid = sessionId;
    const containerEl = containerRef.current;
    if (!containerEl) return;

    // ── Create terminal ──
    const term = new Terminal({
      fontFamily: getTerminalFontFamily(),
      theme: buildThemeFromCSS(),
      convertEol: false,
      cursorBlink: true,
      cursorWidth: 2,
      lineHeight: 1.15,
      letterSpacing: 0,
      fontSize: getTerminalFontSize(),
      cursorStyle: getTerminalCursorStyle() as "bar" | "block" | "underline",
      altClickMovesCursor: true,
    });

    const fitAddon = new FitAddon();
    const serializeAddon = new SerializeAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(serializeAddon);
    term.open(containerEl);

    // ── WebGL renderer ──
    // Offload glyph rendering to the GPU — substantially faster and smoother
    // for the long-running, high-output agent sessions Codemux runs. Must load
    // AFTER term.open() because the addon needs the opened terminal's DOM
    // element. Wrapped in try/catch so an environment without WebGL2 support
    // (e.g. a software-only WebView) falls back to the default DOM renderer
    // instead of throwing and breaking the whole pane.
    let webglAddon: WebglAddon | null = null;
    try {
      const addon = new WebglAddon();
      // Required: when the GPU drops the canvas context, dispose the addon so
      // xterm falls back to the DOM renderer rather than rendering a blank pane.
      addon.onContextLoss(() => {
        addon.dispose();
        webglAddonRef.current = null;
      });
      term.loadAddon(addon);
      webglAddon = addon;
    } catch (err) {
      console.warn(
        "[codemux::terminal] WebGL renderer unavailable, using DOM renderer:",
        err,
      );
      webglAddon = null;
    }

    termRef.current = term;
    fitAddonRef.current = fitAddon;
    serializeAddonRef.current = serializeAddon;
    webglAddonRef.current = webglAddon;
    kittyStackRef.current = [];
    kittyLevelRef.current = 0;

    // ── Custom key handler ──
    term.attachCustomKeyEventHandler((ev) => {
      // Escape / Ctrl+C — clear this pane's Working/Permission status.
      // Claude Code's Stop hook does NOT fire on user interrupts (Ctrl+C, Escape).
      // Check this specific pane's status to avoid false positives (e.g. vim).
      if (ev.type === "keydown" && paneId) {
        const isInterrupt =
          ev.key === "Escape" ||
          (ev.key === "c" && ev.ctrlKey && !ev.shiftKey && !ev.altKey);
        if (isInterrupt) {
          const status =
            useAppStore.getState().appState?.pane_statuses[paneId];
          if (status === "working" || status === "permission") {
            clearAgentStatus(sid).catch(console.error);
          }
          return true; // let the key pass through to the terminal
        }
      }
      // App-level shortcuts — must fire BEFORE CSI u encoding so that
      // Ctrl+K (command palette) etc. are never sent to the terminal.
      if (isAppShortcut(ev)) return false;
      // Modern terminals unconditionally send CSI u for modified
      // functional keys (Enter, Tab, Space). This lets CLI apps like
      // Claude Code distinguish Shift+Enter from Enter without
      // requiring Kitty protocol negotiation.
      // Backspace is only CSI-u-encoded when Kitty mode is active so
      // that backward-kill-word (Ctrl+Backspace → \x17) still works
      // in plain shells.
      {
        const codepoint = KITTY_FUNCTIONAL_KEYS.get(ev.key);
        const mod = csiUModifier(ev);
        if (codepoint !== undefined && mod > 1) {
          const isBackspace = codepoint === BACKSPACE_CODEPOINT;
          if (!isBackspace || kittyLevelRef.current > 0) {
            if (ev.type === "keydown") {
              writeToPty(sid, csiUSequence(codepoint, mod)).catch(console.error);
            }
            ev.preventDefault?.();
            return false;
          }
        }
      }
      // Terminal-level shortcuts (resolved from keybind registry)
      const overrides = useSyncedSettingsStore.getState().settings.keyboard.shortcuts;
      const resolved = resolveKeybinds(overrides);
      const killCombo = resolved.getKeysForAction("backwardKillWord");
      const copyCombo = resolved.getKeysForAction("copySelection");
      const pasteCombo = resolved.getKeysForAction("pasteTerminal");

      if (killCombo && matchesKeyCombo(ev, killCombo)) {
        if (ev.type === "keydown") {
          writeToPty(sid, "\x17").catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      if (copyCombo && matchesKeyCombo(ev, copyCombo)) {
        if (ev.type === "keydown") {
          const selection = term.getSelection();
          if (selection) navigator.clipboard.writeText(selection).catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      if (pasteCombo && matchesKeyCombo(ev, pasteCombo)) {
        if (ev.type === "keydown") {
          navigator.clipboard
            .readText()
            .then((text) => { if (text) term.paste(text); })
            .catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      return true;
    });

    // ── WKWebView newline bug workaround ──
    const blockNewline = (e: Event) => {
      if (kittyLevelRef.current <= 0) return;
      const ie = e as InputEvent;
      if (
        ie.inputType === "insertLineBreak" ||
        ie.inputType === "insertParagraph" ||
        (ie.inputType === "insertText" && ie.data === "\n")
      ) {
        e.stopPropagation();
        e.preventDefault();
      }
    };
    containerEl.addEventListener("input", blockNewline, true);
    blockNewlineRef.current = blockNewline;

    // ── User input handler ──
    let pendingInput = "";
    let inputQueued = false;
    const dataDisposable = term.onData((data) => {
      pendingInput += data;
      if (!inputQueued) {
        inputQueued = true;
        queueMicrotask(() => {
          const batch = pendingInput;
          pendingInput = "";
          inputQueued = false;
          writeToPty(sid, batch).catch((err) => {
            console.error(`Failed to write to PTY for ${sid}:`, err);
          });
        });
      }
    });
    dataDisposableRef.current = dataDisposable;

    // ── Scrollback serialization helper ──
    // Shared between the live-serialize registry (on close) and the unmount
    // cache path (on tab/workspace switch).
    const buildScrollbackPayload = (): ScrollbackPayload | null => {
      const t = termRef.current;
      const sa = serializeAddonRef.current;
      if (!t || !sa) return null;

      const appState = useAppStore.getState().appState;
      if (!appState) return null;

      const session = appState.terminal_sessions.find(
        (s) => s.session_id === sid,
      );
      if (!session) return null;

      // O(1) lookup via the cached session→workspace index. The previous
      // `JSON.stringify(surf.root).includes(sid)` scan was O(panes^2) and
      // ran on every scrollback save / mount — measurable cost with many
      // workspaces. See `buildSessionWorkspaceIndex` in `app-store.ts`.
      const workspaceId = getSessionWorkspaceId(sid);

      // Detect alternate screen buffer (TUI apps: vim, htop, Claude Code, etc.)
      const isAlternateBuffer = t.buffer.active.type === "alternate";

      // Alt-screen content is garbled when serialized and is therefore NEVER
      // restored (the mount path guards on `!meta.alternate_buffer`). Running
      // serializeAddon.serialize on it anyway is pure wasted main-thread work
      // that blocks the workspace switch — and the dominant case here is
      // exactly the long-running TUI agents (Claude Code, lazygit, vim, btop)
      // the user switches between. Skip the serialize for alt-screen panes and
      // persist an empty buffer with the flag set; the live PTY reattach
      // replay reconstructs the screen on return regardless.
      const scrollbackLines =
        useSyncedSettingsStore.getState().settings.session_restore.scrollback_lines;
      const serializeStart = performance.now();
      const data = isAlternateBuffer
        ? ""
        : sa.serialize({ scrollback: scrollbackLines });
      const serializeMs = performance.now() - serializeStart;
      if (serializeMs > 30) {
        // eslint-disable-next-line no-console
        console.info(
          `[codemux::terminal-serialize] sid=${sid.slice(0, 8)} ` +
            `serialize=${serializeMs.toFixed(0)}ms bytes=${data.length} alt=${isAlternateBuffer}`,
        );
      }

      return {
        pane_id: paneId ?? sid,
        session_id: sid,
        workspace_id: workspaceId ?? "",
        working_directory: session.cwd,
        original_command: session.original_command,
        cols: session.cols,
        rows: session.rows,
        data,
        adapter_captures: {
          ...restoredCapturesRef.current,
          ...(session.adapter_captures ?? {}),
        },
        adapter_id: null,
        alternate_buffer: isAlternateBuffer,
      };
    };

    // Register for live serialization on close
    const unregisterSerialize = registerTerminalForSerialize(sid, buildScrollbackPayload);

    // Clear any stale cached scrollback for this session (we're live now)
    uncacheTerminalScrollback(sid).catch(() => {});

    // ── Attach PTY session ──
    //
    // Mount cost on workspace switch was previously dominated by THREE
    // sequential awaited IPCs:
    //
    //   getTerminalScrollback  →  getTerminalStatus  →  attachPtyOutput
    //
    // Each round-trip is on the order of 5–30 ms on Linux WebKitGTK
    // (Tauri's IPC docs do not publish a number; per-call cost is
    // dominated by JSON encode/decode plus the WebKit→GTK→Rust hop).
    // With 2-3 terminals re-mounting on every workspace switch, that
    // stacks into the user-perceived 1-second hitch.
    //
    // Scrollback and status are independent reads — they can run in
    // parallel. The `attachPtyOutput` step must come AFTER scrollback
    // is ENQUEUED, otherwise live PTY output could interleave with the
    // historical bytes (xterm's parser is stateful, so byte order across
    // the boundary matters for the alt-screen / cursor state). Both feed
    // the same ordered write queue, so FIFO drain preserves that order
    // even though the writes are now throttled across macrotasks.
    //
    // Net: 3 sequential round-trips → 1 parallel pair + 1 = effectively
    // 2 round-trips per terminal on switch.
    let cancelled = false;
    const attachStarted = performance.now();

    // ── Throttled write pump + PTY producer back-pressure ──
    //
    // Every byte that reaches xterm — disk scrollback restore, the PTY
    // reattach replay, and steady live output — goes through this single
    // ordered queue, drained a budget-bounded batch per macrotask so a
    // multi-MB reattach replay can't peg the main thread and freeze the
    // workspace switch. See ./terminal-write-pump.ts for the full rationale.
    //
    // The consumer-side throttle alone can't stop a fast producer (`yes`, a
    // verbose build, a runaway agent) from outrunning xterm's ~5–35 MB/s
    // ingest — without a signal back to the producer, the backend's
    // pending_output ring grows until it evicts (drops) the oldest output.
    // So we also watch the pump's queued-byte depth: above HIGH we ask the
    // backend to pause this session's PTY read loop (the child then blocks on
    // write() once the kernel PTY buffer fills — real back-pressure); below
    // LOW we resume. `flowPaused` tracks whether we currently hold that pause
    // so cleanup can always release it (the backend also self-heals via a
    // resume-on-attach + max-park backstop).
    let flowPaused = false;
    const pump = createWritePump((data) => term.write(data), {
      onHighWatermark: () => {
        flowPaused = true;
        pausePtyOutput(sid).catch((err) => {
          // Pause didn't land (e.g. the session just exited). Drop the local
          // guard so cleanup won't issue a spurious resume.
          flowPaused = false;
          console.error(`[codemux] flow-control pause failed for ${sid}:`, err);
        });
      },
      onLowWatermark: () => {
        flowPaused = false;
        resumePtyOutput(sid).catch((err) => {
          console.error(`[codemux] flow-control resume failed for ${sid}:`, err);
        });
      },
    });

    (async () => {
      // O(1) reverse-index lookup; see `buildSessionWorkspaceIndex`.
      const workspaceId = getSessionWorkspaceId(sid);
      const restoreEnabled = useSyncedSettingsStore.getState().settings.session_restore.enabled;
      const wantsScrollback = restoreEnabled && !!workspaceId && !!paneId;

      // Stage 1: parallel reads — scrollback + status. Both are
      // independent disk/state lookups in Rust; running them in
      // parallel halves the wall-clock for the read phase. Either
      // failure is recoverable (we fall back to fresh pane / failed
      // overlay).
      const [scrollbackResult, statusResult] = await Promise.allSettled([
        wantsScrollback
          ? getTerminalScrollback(workspaceId!, paneId!)
          : Promise.resolve(null),
        getTerminalStatus(sid),
      ]);
      if (cancelled) return;

      // Enqueue scrollback first so historical bytes drain before any live
      // channel byte (FIFO order on the shared pump preserves the parser's
      // stateful alt-screen / cursor boundary). Errors here are logged via
      // the catch-all on the IIFE and the pane continues with a fresh xterm.
      if (
        scrollbackResult.status === "fulfilled" &&
        scrollbackResult.value &&
        !cancelled
      ) {
        const scrollback = scrollbackResult.value;
        const { meta } = scrollback;
        // Cache captures from scrollback metadata — these survive tab
        // switches even when layout.json doesn't persist
        // adapter_captures.
        if (meta.adapter_captures && Object.keys(meta.adapter_captures).length > 0) {
          restoredCapturesRef.current = meta.adapter_captures;
        }
        if (!meta.alternate_buffer && scrollback.data) {
          pump.enqueueString(scrollback.data);
          pump.enqueue(
            new TextEncoder().encode(
              "\r\n\x1b[2m── session restored ──\x1b[0m\r\n\r\n",
            ),
          );
        }
      }

      // Update overlay from whichever result we got.
      if (statusResult.status === "fulfilled") {
        updateStatusOverlay(statusResult.value);
      } else {
        updateStatusOverlay({
          session_id: sid,
          state: "failed",
          message: "Failed to read terminal status",
          exit_code: null,
        });
      }

      // Stage 2: attach the channel. Must come AFTER the scrollback is
      // enqueued so live bytes drain behind the historical bytes. The
      // callback advances the kitty stack synchronously (the input handler
      // reads kittyLevel on every keystroke, so it can't wait behind the
      // throttled pump) then enqueues the bytes for the shared drain.
      const channel = new Channel<unknown>((payload) => {
        if (cancelled) return;
        const bytes = extractBytes(payload);
        if (!bytes) return;
        scanKittyProtocol(bytes);
        pump.enqueue(bytes);
      });

      try {
        await attachPtyOutput(sid, channel);
        if (cancelled) return;
        attachedSessionRef.current = sid;
      } catch (err) {
        if (cancelled) return;
        updateStatusOverlay({
          session_id: sid,
          state: "failed",
          message: `Failed to attach terminal output: ${String(err)}`,
          exit_code: null,
        });
        return;
      }

      // Stage 3: fit + resize. resizePty is fire-and-forget so doesn't
      // block the mount finishing.
      fitAddon.fit();
      if (term.cols > 0 && term.rows > 0) {
        resizePty(sid, term.cols, term.rows).catch(console.error);
      }

      // Timing log gated to slow mounts so steady-state stays quiet.
      // The user can grep stderr for `[codemux::terminal-mount]` while
      // exercising workspace switches to see actual numbers.
      const elapsed = performance.now() - attachStarted;
      if (elapsed > 50) {
        // eslint-disable-next-line no-console
        console.info(
          `[codemux::terminal-mount] sid=${sid.slice(0, 8)} attach=${elapsed.toFixed(0)}ms`,
        );
      }
    })();

    // ── ResizeObserver ──
    const shellEl = shellRef.current;
    if (shellEl) {
      const observer = new ResizeObserver(() => {
        if (!visibleRef.current) return;
        if (resizeTimerRef.current !== null) {
          clearTimeout(resizeTimerRef.current);
        }
        resizeTimerRef.current = setTimeout(() => {
          resizeTimerRef.current = null;
          syncTerminalSize();
        }, 150);
      });
      observer.observe(shellEl);
      resizeObserverRef.current = observer;
    }

    // ── Window resize handler ──
    const windowResize = () => {
      if (!visibleRef.current) return;
      if (windowResizeTimerRef.current) clearTimeout(windowResizeTimerRef.current);
      windowResizeTimerRef.current = setTimeout(() => {
        windowResizeTimerRef.current = null;
        syncTerminalSize();
      }, 100);
    };
    window.addEventListener("resize", windowResize);

    // ── Cleanup ──
    return () => {
      cancelled = true;

      if (attachedSessionRef.current) {
        detachPtyOutput(attachedSessionRef.current).catch(console.error);
        attachedSessionRef.current = null;
      }
      dataDisposable.dispose();
      dataDisposableRef.current = null;

      // Stop the throttled pump and drop anything still queued, so an
      // in-flight drain can't write into the terminal we're about to dispose.
      pump.cancel();

      // If we're unmounting while holding a flow-control pause, release it so
      // a backgrounded (daemon-backed) agent isn't left blocked on write().
      // The backend also self-heals (resume-on-attach + max-park backstop),
      // but resuming here makes it immediate.
      if (flowPaused) {
        flowPaused = false;
        resumePtyOutput(sid).catch(console.error);
      }

      containerEl.removeEventListener("input", blockNewline, true);
      blockNewlineRef.current = null;

      if (resizeTimerRef.current !== null) {
        clearTimeout(resizeTimerRef.current);
        resizeTimerRef.current = null;
      }
      if (windowResizeTimerRef.current !== null) {
        clearTimeout(windowResizeTimerRef.current);
        windowResizeTimerRef.current = null;
      }

      resizeObserverRef.current?.disconnect();
      resizeObserverRef.current = null;

      window.removeEventListener("resize", windowResize);

      // Cache scrollback BEFORE disposing xterm — the serialize addon needs
      // the live terminal buffer. This covers tab switches and workspace switches.
      const restoreEnabled = useSyncedSettingsStore.getState().settings.session_restore.enabled;
      if (restoreEnabled) {
        const payload = buildScrollbackPayload();
        if (payload && payload.data) {
          cacheTerminalScrollback(payload).catch(() => {});
        }
      }

      unregisterSerialize();
      // Dispose the WebGL addon before the terminal so its GPU context /
      // canvas is released deterministically (no leak across mounts). The
      // wrapped dispose unregisters it from xterm's addon manager, so the
      // following term.dispose() won't double-dispose it.
      webglAddonRef.current?.dispose();
      webglAddonRef.current = null;
      serializeAddonRef.current = null;
      fitAddonRef.current = null;
      kittyStackRef.current = [];
      kittyLevelRef.current = 0;
      term.dispose();
      termRef.current = null;
    };
    // Intentionally depend only on sessionId — theme updates are handled separately
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // TODO: re-enable as "system theme" option in settings
  // useEffect(() => {
  //   if (termRef.current) {
  //     termRef.current.options.theme = buildXtermTheme(theme);
  //   }
  // }, [theme]);
  //
  // useEffect(() => {
  //   if (termRef.current) {
  //     termRef.current.options.fontFamily = shellAppearance.font_family || "monospace";
  //     fitAddonRef.current?.fit();
  //   }
  // }, [shellAppearance]);

  // ── Re-read CSS variables when theme class/style changes ──
  useEffect(() => {
    const observer = new MutationObserver(() => {
      if (termRef.current) {
        termRef.current.options.theme = buildThemeFromCSS();
      }
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    return () => observer.disconnect();
  }, []);

  // ── Focus management ──
  // Depends on `sessionId` as well as `focused`: when a new tab is created
  // or a preset launches a CLI agent, PaneContainer swaps the active surface
  // but React reuses this same TerminalPane fiber. `focused` stays `true`
  // across that swap (the active pane of both surfaces is focused), so an
  // effect keyed only on `focused` would never re-run — leaving the freshly
  // mounted terminal unfocused until the user clicks it. Keying on
  // `sessionId` re-focuses whenever the underlying terminal changes. The
  // main mount effect is declared earlier, so it has already pointed
  // `termRef.current` at the new terminal by the time this runs.
  useEffect(() => {
    if (focused && termRef.current) {
      termRef.current.focus();
    }
  }, [focused, sessionId]);

  return (
    <div ref={shellRef} className="relative flex flex-1 w-full h-full min-w-0 min-h-0 bg-background">
      <div
        ref={containerRef}
        className="block flex-1 w-full h-full min-w-0 min-h-0 overflow-hidden px-2 py-1.5 box-border [&_.xterm]:h-full [&_.xterm]:w-full [&_.xterm-viewport]:!bg-transparent"
      />
      <div
        ref={statusOverlayRef}
        className="terminal-overlay starting absolute inset-0 z-0 flex items-center justify-center p-6 bg-background/95 backdrop-blur-sm"
        style={{ display: statusRef.current.state === "ready" ? "none" : "flex" }}
      >
        {/* Centered status card. The h2/p/code below are mutated
            DOM-side in updateStatusOverlay() for perf — don't
            change their tags or query selectors without updating
            the mutation code. The spinner is CSS-animated and
            hidden via `[data-state="failed"]` so failed state
            gets the warning dot instead.

            For remote workspaces hitting tunnel timeout, the
            failure message includes a "Try Test Connection /
            Pull back" suggestion (see terminal/mod.rs Failed
            emit path). */}
        <div className="w-full max-w-[420px] rounded-lg border border-border bg-card shadow-lg overflow-hidden">
          <div className="flex items-center gap-3 px-5 py-4 border-b border-border/60">
            {/* Spinner shown for starting state, warning dot for
                failed. CSS-only so DOM mutations on state change
                just toggle the data attribute via className. */}
            <div className="status-indicator relative size-4 shrink-0">
              <div className="spinner absolute inset-0 rounded-full border-2 border-muted border-t-primary animate-spin" />
              <div
                className="warning absolute inset-0 rounded-full bg-destructive/90 hidden items-center justify-center text-[10px] font-bold text-destructive-foreground"
                aria-hidden
              >
                !
              </div>
            </div>
            <h2 className="text-sm font-semibold text-foreground leading-tight">
              Terminal starting
            </h2>
          </div>
          <div className="px-5 py-4 space-y-2">
            <p className="text-sm text-muted-foreground leading-relaxed">
              {statusRef.current.message ?? "Waiting for shell status..."}
            </p>
            <span className="status-meta inline-block text-xs font-mono text-muted-foreground/70" />
          </div>
        </div>
      </div>
    </div>
  );
}
