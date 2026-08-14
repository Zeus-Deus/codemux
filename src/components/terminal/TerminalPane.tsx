import { useEffect, useRef, useCallback, useMemo, memo } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebglAddon } from "@xterm/addon-webgl";
import { shouldLoadWebglAddon } from "./webgl-renderer-probe";
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
import { endSubMeasure, startSubMeasure } from "@/lib/perf/interaction-trace";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import {
  getTerminalCursorStyle,
  selectLegacyTerminalFontFamily,
  useSettingsStore,
} from "@/stores/settings-store";
import { resolveTerminalFontFamily, resolveTypographySettings } from "@/lib/typography";
import { applyTerminalTypography } from "@/lib/terminal-typography";
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
import { createIdleScrollbackSerializer } from "./scrollback-idle-serializer";
import { parkTeardown, flushTeardown } from "./deferred-teardown";
import { useAppStore, getSessionWorkspaceId } from "@/stores/app-store";
import {
  useTerminalCwdStore,
  parseOsc7,
} from "@/stores/terminal-cwd-store";
import { onTerminalStatus } from "@/tauri/events";
import { useSyntaxThemeColors } from "@/hooks/use-theme-colors";
import { themeColorsToXtermTheme } from "@/lib/xterm-theme";
import { useTauriEvent } from "@/hooks/use-tauri-event";
import type { TerminalStatusPayload } from "@/tauri/types";

interface Props {
  sessionId: string;
  paneId?: string;
  focused: boolean;
  visible: boolean;
  title: string;
}

// How long the status overlay takes to fade out once the session is alive
// (state → ready, or first output on a migrating pane). Must match the
// `transition: opacity` duration on `.terminal-overlay` in globals.css so the
// element is only removed from layout (display:none) after the fade completes.
const OVERLAY_FADE_MS = 160;

function extractBytes(payload: unknown): Uint8Array | null {
  if (payload instanceof Uint8Array) return payload;
  if (payload instanceof ArrayBuffer) return new Uint8Array(payload);
  if (Array.isArray(payload)) return new Uint8Array(payload as number[]);
  if (typeof payload === "string") return new TextEncoder().encode(payload);
  return null;
}

/** True while the terminal is showing the alternate screen (vim, htop,
 *  Claude Code, …). Alt-screen content serializes to garbage and is never
 *  cached or restored. */
function isAltScreen(t: Terminal): boolean {
  return t.buffer.active.type === "alternate";
}

// #127: memo is effective because setAppState performs structural sharing —
// PaneNode passes only primitive props (sessionId/paneId/focused/visible/title),
// so the exported pane skips re-render on backend ticks that don't change them.
// Export-only wrapper: the component body below is unchanged.
export const TerminalPane = memo(function TerminalPane({ sessionId, paneId, focused, visible }: Props) {
  const syntaxTheme = useSyntaxThemeColors();
  const typographyAppearance = useSyncedSettingsStore((s) => s.settings.appearance);
  const legacyTerminalFamily = useSettingsStore(selectLegacyTerminalFontFamily);
  const typography = useMemo(
    () => resolveTypographySettings(typographyAppearance),
    [typographyAppearance],
  );
  const terminalFamily = resolveTerminalFontFamily(typography, legacyTerminalFamily);

  // Refs for mutable state that persists across renders
  const shellRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const serializeAddonRef = useRef<SerializeAddon | null>(null);
  const webglAddonRef = useRef<WebglAddon | null>(null);
  const attachedSessionRef = useRef<string | null>(null);
  // Subscriber generation returned by `attachPtyOutput`. Threaded back into
  // detach / pause / resume so a stale teardown only ever targets this pane's
  // own subscriber (and never a mirror consumer that attached the same
  // session). `null` until the attach resolves.
  const attachedGenerationRef = useRef<number | null>(null);
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
  // Pending display:none after a fade-out, so a status flip mid-fade can cancel it.
  const overlayHideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
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
      // Session is alive — fade the overlay out, then drop it from layout once
      // the opacity transition finishes so the hidden card can't swallow clicks
      // on the live terminal underneath. Skip if already hidden (steady-state
      // Ready re-emits) so we don't restart the fade on every keystroke echo.
      if (overlayHideTimerRef.current === null && el.style.display !== "none") {
        el.style.opacity = "0";
        overlayHideTimerRef.current = setTimeout(() => {
          overlayHideTimerRef.current = null;
          // Guard against a flip back to a visible state mid-fade (e.g. a fast
          // exit right after ready) — only hide if we're still ready.
          if (statusRef.current.state === "ready") el.style.display = "none";
        }, OVERLAY_FADE_MS);
      }
      return;
    }
    // A visible state arrived — cancel any in-flight fade-out and show now.
    if (overlayHideTimerRef.current !== null) {
      clearTimeout(overlayHideTimerRef.current);
      overlayHideTimerRef.current = null;
    }
    el.style.display = "flex";
    el.style.opacity = "1";
    // Keep the base classes (positioning, backdrop) and append
    // the state for any state-specific CSS hooks downstream.
    el.className = `terminal-overlay ${status.state} absolute inset-0 z-0 flex items-center justify-center p-6 bg-background/95 backdrop-blur-sm`;
    const failed = status.state === "failed";
    // Migrating reads as a deliberate transition, not a fresh shell start, so it
    // gets its own heading; it keeps the spinner (it's an in-progress state).
    const migrating = status.state === "migrating";
    // Swap spinner vs warning indicator visibility.
    const spinner = el.querySelector<HTMLElement>(".status-indicator .spinner");
    const warning = el.querySelector<HTMLElement>(".status-indicator .warning");
    if (spinner) spinner.style.display = failed ? "none" : "block";
    if (warning) warning.style.display = failed ? "flex" : "none";
    const h2 = el.querySelector("h2");
    const p = el.querySelector("p");
    const code = el.querySelector(".status-meta");
    if (h2)
      h2.textContent = failed
        ? "Terminal unavailable"
        : migrating
          ? "Migrating workspace"
          : "Terminal starting";
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

    // Switching back to a session whose teardown is still parked (A → B → A):
    // finish it NOW, before this mount builds a second Terminal for the same
    // session. Two invariants depend on it — the parked job's serialize +
    // `cacheTerminalScrollback` must land before the read below, and its WebGL
    // context must be released before we allocate the replacement (see the
    // context-cap note in `deferred-teardown.ts`).
    flushTeardown(sid);

    const containerEl = containerRef.current;
    if (!containerEl) return;

    // ── Create terminal ──
    const term = new Terminal({
      fontFamily: terminalFamily,
      theme: themeColorsToXtermTheme(syntaxTheme),
      convertEol: false,
      cursorBlink: true,
      cursorWidth: 2,
      lineHeight: 1.15,
      letterSpacing: 0,
      fontSize: typography.terminalSize,
      cursorStyle: getTerminalCursorStyle() as "bar" | "block" | "underline",
      altClickMovesCursor: true,
    });

    const fitAddon = new FitAddon();
    const serializeAddon = new SerializeAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(serializeAddon);
    term.open(containerEl);

    // ── WebGL renderer (hardware GL only) ──
    // Offload glyph rendering to the GPU — substantially faster and smoother
    // for the long-running, high-output agent sessions Codemux runs. Must load
    // AFTER term.open() because the addon needs the opened terminal's DOM
    // element. Gated on a one-time probe (`webgl-renderer-probe.ts`): when the
    // WebView's WebGL2 is backed by a software rasterizer (SwiftShader,
    // llvmpipe, …) the addon *constructs fine* but rasterizes every frame on
    // the CPU, which adds per-keystroke input latency vs the DOM renderer —
    // the v0.9.0 typing-lag regression. Linux WebKitGTK is declined outright
    // (masked renderer strings + documented input-lag history; see the probe
    // module for the full policy and the localStorage escape hatch). Also
    // wrapped in try/catch so an environment without WebGL2 support falls
    // back to the default DOM renderer instead of throwing and breaking the
    // whole pane.
    let webglAddon: WebglAddon | null = null;
    if (shouldLoadWebglAddon().use) {
      try {
        const addon = new WebglAddon();
        // Required: when the GPU drops the canvas context, dispose the addon so
        // xterm falls back to the DOM renderer rather than rendering a blank pane.
        addon.onContextLoss(() => {
          addon.dispose();
          // Clear the local too: the deferred teardown disposes from this
          // binding (the ref is nulled at unmount, before the job runs) and
          // must not double-dispose an addon the GPU already took away.
          webglAddon = null;
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

    // ── OSC 7: shell-reported working directory ──
    // Shells with integration (fish, vte-patched bash, kitty/zsh setups)
    // emit `ESC ] 7 ; file://host/path ST` on every prompt. Tapping it here
    // is free — the sequence already arrives on the wire and xterm silently
    // drops it today — and it's the only cwd source that works for
    // remote/SSH panes, where the shell's pid lives on another machine.
    // Sessions whose shell stays quiet are covered by the `/proc` poller
    // instead (see `use-terminal-cwd-poll.ts`).
    //
    // Returning false lets xterm continue its default handling of the
    // sequence rather than swallowing it, keeping us a passive observer.
    const osc7Disposable = term.parser.registerOscHandler(7, (payload) => {
      const cwd = parseOsc7(payload);
      // `getState()` rather than a subscribed selector: this pane must not
      // re-render when its own cwd changes — only the header in `PaneNode`
      // subscribes, and a `cd` shouldn't churn the xterm instance.
      if (cwd) useTerminalCwdStore.getState().setCwd(sid, cwd, "osc7");
      return false;
    });

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
    //
    // `precomputedData` lets the caller skip the expensive `serialize()` call
    // entirely by supplying an already-serialized buffer — the idle serializer
    // below opportunistically produces one while the pane is quiet so the
    // latency-critical unmount/close path doesn't have to pay for it (issue
    // #128). A precomputed buffer is ALWAYS primary-screen content (the idle
    // cache is only ever taken off the primary screen and only handed back when
    // we're still on it), so we persist it with `alternate_buffer: false`. All
    // other payload metadata (session lookup, workspace id, cwd, cols/rows,
    // adapter captures) is still recomputed fresh at call time. `trigger` only
    // tags the >30ms timing log so the two call sites are distinguishable.
    // One timed serialize shared by the idle path and the synchronous
    // fallback in buildScrollbackPayload. Only the serialize + timing is
    // unified: the two `[codemux::terminal-serialize]` log lines differ per
    // trigger (idle has no `alt=` field, and DEV gating differs) and are an
    // observability contract asserted by e2e, so each caller emits its own.
    const timedSerialize = (
      sa: SerializeAddon,
    ): { data: string; ms: number; lines: number } => {
      const lines =
        useSyncedSettingsStore.getState().settings.session_restore
          .scrollback_lines;
      const start = performance.now();
      const data = sa.serialize({ scrollback: lines });
      return { data, ms: performance.now() - start, lines };
    };

    const buildScrollbackPayload = (
      opts: { precomputedData?: string; trigger: "unmount" | "close" },
    ): ScrollbackPayload | null => {
      // The effect's own terminal, not `termRef.current`: the unmount path
      // clears the refs synchronously and finishes the serialize later, from a
      // parked job (see the cleanup below). Both bindings are identical while
      // the pane is mounted, so nothing changes for the live paths.
      const t = term;
      const sa = serializeAddon;

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

      const pre = opts.precomputedData;

      // Detect alternate screen buffer (TUI apps: vim, htop, Claude Code, etc.).
      // Irrelevant when we already hold a precomputed (primary-screen) buffer.
      const isAlternateBuffer = pre !== undefined ? false : isAltScreen(t);

      // Alt-screen content is garbled when serialized and is therefore NEVER
      // restored (the mount path guards on `!meta.alternate_buffer`). Running
      // serializeAddon.serialize on it anyway is pure wasted main-thread work
      // that blocks the workspace switch — and the dominant case here is
      // exactly the long-running TUI agents (Claude Code, lazygit, vim, btop)
      // the user switches between. Skip the serialize for alt-screen panes and
      // persist an empty buffer with the flag set; the live PTY reattach
      // replay reconstructs the screen on return regardless.
      let data: string;
      if (pre !== undefined) {
        // Reuse the idle-cached serialization — skip serialize() entirely.
        data = pre;
      } else if (isAlternateBuffer) {
        data = "";
      } else {
        const { data: freshData, ms } = timedSerialize(sa);
        data = freshData;
        if (ms > 30) {
          // eslint-disable-next-line no-console
          console.info(
            `[codemux::terminal-serialize] sid=${sid.slice(0, 8)} ` +
              `trigger=${opts.trigger} serialize=${ms.toFixed(0)}ms ` +
              `bytes=${data.length} alt=${isAlternateBuffer}`,
          );
        }
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

    // ── Idle-time scrollback serializer ──
    // Opportunistically serializes the buffer while the pane is quiet so the
    // unmount/close path can reuse the result instead of blocking the workspace
    // switch on a synchronous serialize (issue #128). Closes over this effect's
    // terminal + serialize addon, so it stays valid for the deferred teardown.
    // The idle cache bakes in the scrollback_lines value used at serialize
    // time, and a synced-settings change emits no output and no resize — so
    // nothing else invalidates the cache. Track the value each idle serialize
    // used; buildFreshOrCached treats the cache as stale when the current
    // setting differs (otherwise a switch within one idle cycle would persist
    // with the old line count).
    let idleSerializedScrollbackLines = -1;

    const idleSerializer = createIdleScrollbackSerializer({
      serialize: () => {
        const { data, ms, lines } = timedSerialize(serializeAddon);
        idleSerializedScrollbackLines = lines;
        if (import.meta.env.DEV || ms > 30) {
          // DEV observability: every idle serialize. Prod noise unchanged:
          // only the existing >30ms warning, now tagged trigger=idle — a
          // pathological serialize is worth surfacing even off the critical
          // path.
          // eslint-disable-next-line no-console
          console.info(
            `[codemux::terminal-serialize] sid=${sid.slice(0, 8)} ` +
              `trigger=idle serialize=${ms.toFixed(0)}ms bytes=${data.length}`,
          );
        }
        return data;
      },
      // Same reason as buildScrollbackPayload: read the effect's terminal so a
      // deferred teardown still sees the real buffer type after the refs are
      // cleared.
      isAlternateBuffer: () => isAltScreen(term),
      // When session restore is off the unmount path discards the payload, so
      // an idle serialize would be pure wasted main-thread work — gate the
      // idle path on the same setting.
      isEnabled: () =>
        useSyncedSettingsStore.getState().settings.session_restore.enabled,
    });

    // A resize reflows the buffer, invalidating any cached serialization — feed
    // it so the idle cache re-serializes after the new geometry settles.
    const resizeDisposable = term.onResize(() => idleSerializer.notifyResize());

    // `term.write` is async: the pump-side notifyOutput below fires when a
    // chunk is HANDED to xterm, but the parser can still hold unparsed
    // backlog when the 1s settle window elapses — the idle cache would latch
    // "clean" against a buffer that's still about to change. onWriteParsed
    // fires at most once per frame, after data parsing completes, and can
    // fire while writes are still pending — so while backlog remains, each
    // frame's parse slice refreshes lastOutputAt and the settle window can't
    // elapse mid-parse. The pair of hooks covers both boundaries:
    // written-but-not-yet-parsed (pump) and the parse tail (here).
    const writeParsedDisposable = term.onWriteParsed(() =>
      idleSerializer.notifyOutput(),
    );

    // Fresh-if-clean: reuse the idle cache when it's still up to date, else fall
    // back to a fresh synchronous serialize so persistence never regresses.
    const buildFreshOrCached = (
      trigger: "unmount" | "close",
    ): ScrollbackPayload | null => {
      let fresh = idleSerializer.getFreshData();
      if (
        fresh !== null &&
        useSyncedSettingsStore.getState().settings.session_restore
          .scrollback_lines !== idleSerializedScrollbackLines
      ) {
        // scrollback_lines changed since the cache was taken. Settings
        // changes emit no output/resize, so the serializer can't observe
        // them — without this check the switch would persist a payload built
        // with the stale line count.
        fresh = null;
      }
      const payload = buildScrollbackPayload({
        precomputedData: fresh?.data,
        trigger,
      });
      if (import.meta.env.DEV) {
        // eslint-disable-next-line no-console
        console.info(
          `[codemux::terminal-serialize] sid=${sid.slice(0, 8)} ` +
            `trigger=${trigger} reused=${fresh !== null} ` +
            `bytes=${payload?.data.length ?? 0}`,
        );
      }
      return payload;
    };

    // Register for live serialization on close (app quit). Uses the same
    // fresh-if-clean dance so a quit right after an idle serialize is cheap.
    const unregisterSerialize = registerTerminalForSerialize(sid, () =>
      buildFreshOrCached("close"),
    );

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
    // Every byte written to xterm flows through here — live PTY output, the
    // reattach replay, AND the disk-scrollback restore — so this is the single
    // choke point that feeds the idle serializer's dirty tracking. Keep the
    // notify O(1): it's on the per-keystroke local-echo path too.
    const pump = createWritePump((data) => {
      term.write(data);
      idleSerializer.notifyOutput();
    }, {
      onHighWatermark: () => {
        flowPaused = true;
        // Before attach resolves this pane is not yet a subscriber (and the
        // reader isn't feeding it live bytes — only finite disk scrollback is
        // in the pump), so there is nothing to back-pressure yet. The intent is
        // recorded in `flowPaused` and reconciled once we hold a generation.
        const generation = attachedGenerationRef.current;
        if (generation == null) return;
        pausePtyOutput(sid, generation).catch((err) => {
          // Pause didn't land (e.g. the session just exited). Drop the local
          // guard so cleanup won't issue a spurious resume.
          flowPaused = false;
          console.error(`[codemux] flow-control pause failed for ${sid}:`, err);
        });
      },
      onLowWatermark: () => {
        flowPaused = false;
        const generation = attachedGenerationRef.current;
        if (generation == null) return;
        resumePtyOutput(sid, generation).catch((err) => {
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
        // First live byte on a migrating pane means the replacement PTY is
        // producing output — dismiss the "Switching to <host>…" overlay even if
        // the Ready lifecycle event hasn't landed yet (the two ride different
        // IPC paths, so output can win the race). Cheap string compare per chunk.
        if (statusRef.current.state === "migrating") {
          updateStatusOverlay({
            session_id: sid,
            state: "ready",
            message: null,
            exit_code: null,
          });
        }
        scanKittyProtocol(bytes);
        pump.enqueue(bytes);
      });

      try {
        const generation = (await attachPtyOutput(sid, channel)) ?? null;
        if (cancelled) {
          // Unmounted during the attach round-trip: the cleanup below ran
          // before we recorded the generation, so tear down the subscriber we
          // just installed here — otherwise it lingers in the backend fan-out
          // set (dropping bytes via the `cancelled` guard) until the session
          // closes.
          if (generation != null) {
            detachPtyOutput(sid, generation).catch(console.error);
          }
          return;
        }
        attachedSessionRef.current = sid;
        attachedGenerationRef.current = generation;
        // Reconcile a pause that crossed the HIGH watermark during the
        // pre-attach scrollback restore, when we had no generation to attribute
        // it to. Now that we're a subscriber, apply it.
        if (flowPaused && generation != null) {
          pausePtyOutput(sid, generation).catch((err) => {
            flowPaused = false;
            console.error(`[codemux] flow-control pause failed for ${sid}:`, err);
          });
        }
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

      const attachedGeneration = attachedGenerationRef.current;
      if (attachedSessionRef.current && attachedGeneration != null) {
        // Detaching this subscriber (by generation) already drops its pause
        // request via the backend recompute — but only removes THIS pane's
        // subscriber, leaving any mirror consumer streaming.
        detachPtyOutput(attachedSessionRef.current, attachedGeneration).catch(
          console.error,
        );
      }
      attachedSessionRef.current = null;
      attachedGenerationRef.current = null;
      dataDisposable.dispose();
      dataDisposableRef.current = null;

      // Stop the throttled pump and drop anything still queued, so an
      // in-flight drain can't write into the terminal we're about to dispose.
      pump.cancel();

      // If we're unmounting while holding a flow-control pause, release it so
      // a backgrounded (daemon-backed) agent isn't left blocked on write().
      // The detach above already recomputes back-pressure (a removed
      // subscriber's pause dies with it), and the backend self-heals besides
      // (resume-on-attach + max-park backstop); this is belt-and-suspenders and
      // only meaningful if we somehow paused without a live subscriber.
      if (flowPaused) {
        flowPaused = false;
        if (attachedGeneration != null) {
          resumePtyOutput(sid, attachedGeneration).catch(console.error);
        }
      }

      resizeDisposable.dispose();
      writeParsedDisposable.dispose();
      osc7Disposable.dispose();

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
      if (overlayHideTimerRef.current !== null) {
        clearTimeout(overlayHideTimerRef.current);
        overlayHideTimerRef.current = null;
      }

      resizeObserverRef.current?.disconnect();
      resizeObserverRef.current = null;

      window.removeEventListener("resize", windowResize);

      // ── Deferred tail: serialize + persist scrollback, then dispose ──
      //
      // Everything above stops output, input and event listening, and has to
      // stay synchronous. What is left is pure cost that the incoming pane
      // cannot observe: when the idle cache is stale, `buildFreshOrCached`
      // runs a synchronous xterm serialize (the >30ms hotspot of issue #128)
      // inside the switch that is trying to paint, and the disposes aren't
      // free either. Park it and run it after the next paint, at idle.
      //
      // The job closes over the effect's own `term` / `serializeAddon` /
      // `webglAddon` (never the refs, which are cleared just below) so React
      // state can't reach the parked instance, and it is bounded on both of
      // this area's historical failure modes — a parked terminal is already
      // detached from the PTY so it consumes no output, and it still disposes
      // its WebGL addon under a hard 2-job cap. See `deferred-teardown.ts`.
      //
      // With session restore off there is nothing to serialize, but the job is
      // still parked: the disposes are the remaining cost, and one path keeps
      // the WebGL-context bound identical in both settings states.
      const restoreEnabled = useSyncedSettingsStore.getState().settings.session_restore.enabled;
      parkTeardown({
        sessionId: sid,
        run: () => {
          // Serialize BEFORE disposing xterm — the serialize addon reads the
          // terminal buffer, which is still intact in memory even though the
          // container is out of the DOM. Fresh-if-clean: reuse the idle
          // serializer's buffer when it is still current, else serialize now.
          if (restoreEnabled) {
            // Same measure name as before this became deferred, so a
            // before/after comparison of `terminal-teardown` holds. Off the
            // critical path there is no open interaction to attribute to and
            // the sample is dropped by design — which is itself the signal
            // that teardown left the switch. A synchronous flush (fast switch
            // back, or queue eviction) still lands on the open trace.
            const teardownStarted = startSubMeasure();
            const payload = buildFreshOrCached("unmount");
            endSubMeasure("terminal-teardown", teardownStarted);
            if (payload && payload.data) {
              cacheTerminalScrollback(payload).catch(() => {});
            }
          }

          // Dispose the idle serializer BEFORE term.dispose() so a pending idle
          // callback can't serialize into a torn-down terminal.
          idleSerializer.dispose();

          // Unregistered here rather than synchronously so an app quit during
          // the parked window still finds a live terminal to serialize. Only a
          // remount of this same session could collide on the registry key, and
          // that path flushes this job before it registers.
          unregisterSerialize();
          // Dispose the WebGL addon before the terminal so its GPU context /
          // canvas is released deterministically (no leak across mounts). The
          // wrapped dispose unregisters it from xterm's addon manager, so the
          // following term.dispose() won't double-dispose it.
          webglAddon?.dispose();
          webglAddon = null;
          term.dispose();
        },
      });

      webglAddonRef.current = null;
      serializeAddonRef.current = null;
      fitAddonRef.current = null;
      kittyStackRef.current = [];
      kittyLevelRef.current = 0;
      termRef.current = null;
    };
    // Intentionally depend only on sessionId — theme updates are handled separately
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // One palette drives xterm, Shiki, and CodeMirror. App-theme changes and
  // desktop-theme events both flow through useSyntaxThemeColors.
  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = themeColorsToXtermTheme(syntaxTheme);
  }, [syntaxTheme]);

  // Font changes are live: update the existing xterm instance, clear its
  // renderer atlas, refit the cell grid, and tell the PTY its new rows /
  // columns. Recreating the terminal would lose modes, selection, and scroll
  // position — the state users are looking at while tuning type.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    if (!applyTerminalTypography(term, terminalFamily, typography.terminalSize)) return;
    const frame = requestAnimationFrame(() => {
      void syncTerminalSize();
    });
    return () => cancelAnimationFrame(frame);
  }, [syncTerminalSize, terminalFamily, typography.terminalSize]);

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
});
