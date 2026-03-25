import { useEffect, useRef, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import type { ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { isAppShortcut } from "@/lib/app-shortcuts";
import {
  writeToPty,
  resizePty,
  detachPtyOutput,
  attachPtyOutput,
  getTerminalStatus,
  Channel,
} from "@/tauri/commands";
import { onTerminalStatus } from "@/tauri/events";
// TODO: re-enable as "system theme" option in settings
// import { useThemeColors } from "@/hooks/use-theme-colors";
import { useTauriEvent } from "@/hooks/use-tauri-event";
import type { TerminalStatusPayload } from "@/tauri/types";

interface Props {
  sessionId: string;
  focused: boolean;
  visible: boolean;
  title: string;
}

// Static ANSI palette — doesn't change with presets
const ANSI_COLORS = {
  black: "#09090b",
  red: "#ef4444",
  green: "#22c55e",
  yellow: "#eab308",
  blue: "#3b82f6",
  magenta: "#a855f7",
  cyan: "#06b6d4",
  white: "#fafafa",
  brightBlack: "#52525b",
  brightRed: "#f87171",
  brightGreen: "#4ade80",
  brightYellow: "#facc15",
  brightBlue: "#60a5fa",
  brightMagenta: "#c084fc",
  brightCyan: "#22d3ee",
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
    cursor: getCSSVar("--foreground"),
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

export function TerminalPane({ sessionId, focused, visible }: Props) {
  // TODO: re-enable as "system theme" option in settings
  // const { theme, shellAppearance } = useThemeColors();

  // Refs for mutable state that persists across renders
  const shellRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const attachedSessionRef = useRef<string | null>(null);
  const kittyLevelRef = useRef(0);
  const pendingPtyWrites = useRef<Uint8Array[]>([]);
  const ptyWriteFrameRef = useRef<number | null>(null);
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

    if (str.includes("\x1b[?u")) {
      writeToPty(sessionIdRef.current, "\x1b[?0u").catch(console.error);
    }

    const pushes = (str.match(/\x1b\[>[0-9]+u/g) ?? []).length;
    const pops = (str.match(/\x1b\[<u/g) ?? []).length;
    kittyLevelRef.current = Math.max(
      0,
      kittyLevelRef.current + pushes - pops,
    );
  }, []);

  // ── PTY output batching ──
  const flushPtyWrites = useCallback(() => {
    ptyWriteFrameRef.current = null;
    const term = termRef.current;
    const pending = pendingPtyWrites.current;
    if (!term || pending.length === 0) return;

    if (pending.length === 1) {
      term.write(pending[0]);
    } else {
      let totalLen = 0;
      for (const chunk of pending) totalLen += chunk.length;
      const combined = new Uint8Array(totalLen);
      let offset = 0;
      for (const chunk of pending) {
        combined.set(chunk, offset);
        offset += chunk.length;
      }
      term.write(combined);
    }
    pendingPtyWrites.current = [];
  }, []);

  const writePtyChunk = useCallback(
    (payload: unknown) => {
      if (!termRef.current) return;
      const bytes = extractBytes(payload);
      if (!bytes) return;

      scanKittyProtocol(bytes);
      pendingPtyWrites.current.push(bytes);
      if (ptyWriteFrameRef.current === null) {
        ptyWriteFrameRef.current = requestAnimationFrame(flushPtyWrites);
      }
    },
    [scanKittyProtocol, flushPtyWrites],
  );

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
  const updateStatusOverlay = useCallback((status: TerminalStatusPayload) => {
    statusRef.current = status;
    const el = statusOverlayRef.current;
    if (!el) return;
    if (status.state === "ready") {
      el.style.display = "none";
    } else {
      el.style.display = "flex";
      el.className = `terminal-overlay ${status.state}`;
      const h2 = el.querySelector("h2");
      const p = el.querySelector("p");
      const code = el.querySelector(".status-meta");
      if (h2)
        h2.textContent =
          status.state === "failed"
            ? "Terminal unavailable"
            : "Terminal starting";
      if (p) p.textContent = status.message ?? "Waiting for shell status...";
      if (code)
        code.textContent =
          status.exit_code !== null ? `Exit code: ${status.exit_code}` : "";
    }
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
      fontFamily: "'JetBrains Mono Variable', monospace",
      theme: buildThemeFromCSS(),
      convertEol: true,
      cursorBlink: true,
      cursorWidth: 2,
      lineHeight: 1.15,
      letterSpacing: 0,
      fontSize: 13,
      cursorStyle: "bar",
      altClickMovesCursor: true,
    });

    const fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(containerEl);

    termRef.current = term;
    fitAddonRef.current = fitAddon;
    kittyLevelRef.current = 0;

    // ── Custom key handler ──
    term.attachCustomKeyEventHandler((ev) => {
      // Shift+Enter
      if (ev.shiftKey && ev.key === "Enter") {
        if (kittyLevelRef.current > 0) {
          if (ev.type === "keydown") {
            writeToPty(sid, "\x1b[13;2u").catch(console.error);
          }
          ev.preventDefault?.();
          return false;
        }
        return true;
      }
      // Ctrl+Backspace → Ctrl+W
      if (ev.ctrlKey && ev.key === "Backspace") {
        if (ev.type === "keydown") {
          writeToPty(sid, "\x17").catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      // Ctrl+Shift+C → copy
      if (ev.ctrlKey && ev.shiftKey && ev.key === "C") {
        if (ev.type === "keydown") {
          const selection = term.getSelection();
          if (selection) navigator.clipboard.writeText(selection).catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      // Ctrl+Shift+V → paste
      if (ev.ctrlKey && ev.shiftKey && ev.key === "V") {
        if (ev.type === "keydown") {
          navigator.clipboard
            .readText()
            .then((text) => { if (text) term.paste(text); })
            .catch(console.error);
        }
        ev.preventDefault?.();
        return false;
      }
      // App-level shortcuts — let them bubble to window handlers
      if (isAppShortcut(ev)) return false;
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

    // ── Attach PTY session ──
    let cancelled = false;
    (async () => {
      try {
        const status = await getTerminalStatus(sid);
        if (cancelled) return;
        updateStatusOverlay(status);
      } catch {
        if (cancelled) return;
        updateStatusOverlay({
          session_id: sid,
          state: "failed",
          message: "Failed to read terminal status",
          exit_code: null,
        });
      }

      const channel = new Channel<unknown>((payload) => {
        writePtyChunk(payload);
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

      // Initial size sync
      fitAddon.fit();
      if (term.cols > 0 && term.rows > 0) {
        resizePty(sid, term.cols, term.rows).catch(console.error);
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

      if (ptyWriteFrameRef.current !== null) {
        cancelAnimationFrame(ptyWriteFrameRef.current);
        ptyWriteFrameRef.current = null;
      }
      pendingPtyWrites.current = [];

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

      fitAddonRef.current = null;
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
  useEffect(() => {
    if (focused && termRef.current) {
      termRef.current.focus();
    }
  }, [focused]);

  return (
    <div ref={shellRef} className="relative flex flex-1 w-full h-full min-w-0 min-h-0 bg-background">
      <div
        ref={containerRef}
        className="block flex-1 w-full h-full min-w-0 min-h-0 overflow-hidden px-2 py-1.5 box-border [&_.xterm]:h-full [&_.xterm]:w-full [&_.xterm-viewport]:!bg-transparent"
      />
      <div
        ref={statusOverlayRef}
        className="terminal-overlay starting absolute inset-0 flex items-center justify-center p-4 bg-background/90"
        style={{ display: statusRef.current.state === "ready" ? "none" : "flex" }}
      >
        <div className="w-full max-w-[440px] p-4 border border-border rounded-sm bg-card">
          <h2 className="mb-2 text-sm font-semibold text-foreground">
            Terminal starting
          </h2>
          <p className="text-sm text-muted-foreground">
            {statusRef.current.message ?? "Waiting for shell status..."}
          </p>
          <span className="status-meta mt-3 inline-block text-xs text-primary" />
        </div>
      </div>
    </div>
  );
}
