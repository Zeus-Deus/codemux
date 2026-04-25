import { useEffect, useRef, useCallback } from "react";
import type { ITheme } from "@xterm/xterm";
import { isAppShortcut } from "@/lib/app-shortcuts";
import { matchesKeyCombo } from "@/lib/keybind-utils";
import {
  KITTY_FUNCTIONAL_KEYS,
  BACKSPACE_CODEPOINT,
  csiUModifier,
  csiUSequence,
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
  getTerminalStatus,
  clearAgentStatus,
} from "@/tauri/commands";
import {
  getOrCreateTerminal,
  attachToContainer,
  detachFromContainer,
  type CachedTerminal,
} from "@/components/terminal/terminal-cache";
import { useAppStore } from "@/stores/app-store";
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

export function TerminalPane({ sessionId, paneId, focused, visible }: Props) {
  // Refs for DOM and per-mount state. The xterm Terminal instance, addons,
  // PTY channel, and serialize registration live in the module-level cache
  // (see terminal-cache.ts) so they survive workspace switches.
  const shellRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const entryRef = useRef<CachedTerminal | null>(null);
  const statusRef = useRef<TerminalStatusPayload>({
    session_id: sessionId,
    state: "starting",
    message: "Starting shell...",
    exit_code: null,
  });
  const statusOverlayRef = useRef<HTMLDivElement>(null);
  const blockNewlineRef = useRef<((e: Event) => void) | null>(null);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const windowResizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const sessionIdRef = useRef(sessionId);
  sessionIdRef.current = sessionId;
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const paneIdRef = useRef(paneId);
  paneIdRef.current = paneId;

  // ── Resize sync ──
  // Only emits resizePty when dims actually changed (no-op detection).
  // No-op when the container has zero dims — the ResizeObserver will fire
  // again once layout completes.
  const syncTerminalSize = useCallback(async () => {
    const entry = entryRef.current;
    const containerEl = containerRef.current;
    if (!entry || !containerEl || !visibleRef.current) return;
    if (containerEl.clientWidth === 0 || containerEl.clientHeight === 0) return;

    entry.fitAddon.fit();
    const { cols, rows } = entry.terminal;
    if (cols === 0 || rows === 0) return;
    if (cols === entry.lastDims.cols && rows === entry.lastDims.rows) return;

    entry.lastDims = { cols, rows };
    try {
      await resizePty(sessionIdRef.current, cols, rows);
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

    let cancelled = false;

    (async () => {
      let entry: CachedTerminal;
      let isNew: boolean;
      try {
        const result = await getOrCreateTerminal(sid, {
          paneId: paneIdRef.current ?? null,
          fontFamily: getTerminalFontFamily(),
          fontSize: getTerminalFontSize(),
          cursorStyle: getTerminalCursorStyle() as
            | "bar"
            | "block"
            | "underline",
          theme: buildThemeFromCSS(),
        });
        entry = result.entry;
        isNew = result.isNew;
      } catch (err) {
        if (cancelled) return;
        updateStatusOverlay({
          session_id: sid,
          state: "failed",
          message: `Failed to initialize terminal: ${String(err)}`,
          exit_code: null,
        });
        return;
      }
      if (cancelled) {
        // The component unmounted during async init. Park the wrapper.
        detachFromContainer(sid);
        return;
      }
      // Race: the session may have been GC'd by useTerminalCacheGc while we
      // were awaiting (PTY exit, close_pane, workspace deletion).
      // Calling methods on a disposed Terminal throws.
      if (entry.disposed) return;

      entryRef.current = entry;
      attachToContainer(sid, containerEl);

      // Custom key handler depends on paneId, which can plausibly differ
      // between mounts of the same session (it doesn't today, but the cache
      // shouldn't pin paneId to a single value). Re-register on each mount.
      entry.terminal.attachCustomKeyEventHandler((ev) => {
        if (ev.type === "keydown" && paneIdRef.current) {
          const isInterrupt =
            ev.key === "Escape" ||
            (ev.key === "c" && ev.ctrlKey && !ev.shiftKey && !ev.altKey);
          if (isInterrupt) {
            const status =
              useAppStore.getState().appState?.pane_statuses[paneIdRef.current];
            if (status === "working" || status === "permission") {
              clearAgentStatus(sid).catch(console.error);
            }
            return true;
          }
        }
        if (isAppShortcut(ev)) return false;
        {
          const codepoint = KITTY_FUNCTIONAL_KEYS.get(ev.key);
          const mod = csiUModifier(ev);
          if (codepoint !== undefined && mod > 1) {
            const isBackspace = codepoint === BACKSPACE_CODEPOINT;
            if (!isBackspace || entry.kittyLevel > 0) {
              if (ev.type === "keydown") {
                writeToPty(sid, csiUSequence(codepoint, mod)).catch(
                  console.error,
                );
              }
              ev.preventDefault?.();
              return false;
            }
          }
        }
        const overrides =
          useSyncedSettingsStore.getState().settings.keyboard.shortcuts;
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
            const selection = entry.terminal.getSelection();
            if (selection)
              navigator.clipboard.writeText(selection).catch(console.error);
          }
          ev.preventDefault?.();
          return false;
        }
        if (pasteCombo && matchesKeyCombo(ev, pasteCombo)) {
          if (ev.type === "keydown") {
            navigator.clipboard
              .readText()
              .then((text) => {
                if (text) entry.terminal.paste(text);
              })
              .catch(console.error);
          }
          ev.preventDefault?.();
          return false;
        }
        return true;
      });

      // ── WKWebView newline bug workaround ──
      const blockNewline = (e: Event) => {
        if (entry.kittyLevel <= 0) return;
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

      // Refresh status from backend (catches state changes that happened
      // while this component was unmounted).
      try {
        const status = await getTerminalStatus(sid);
        if (!cancelled) updateStatusOverlay(status);
      } catch {
        if (!cancelled && isNew) {
          updateStatusOverlay({
            session_id: sid,
            state: "failed",
            message: "Failed to read terminal status",
            exit_code: null,
          });
        }
      }

      // Initial fit + resize. Backend already knows the prior dims; we only
      // emit resize if the container has actual dims and they differ.
      syncTerminalSize();

      if (focused) entry.terminal.focus();
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
      if (windowResizeTimerRef.current)
        clearTimeout(windowResizeTimerRef.current);
      windowResizeTimerRef.current = setTimeout(() => {
        windowResizeTimerRef.current = null;
        syncTerminalSize();
      }, 100);
    };
    window.addEventListener("resize", windowResize);

    // ── Cleanup ──
    // IMPORTANT: do NOT term.dispose() and do NOT detachPtyOutput here.
    // The terminal stays alive in the cache; we only park its wrapper so the
    // React DOM can be collected. The xterm keeps consuming PTY bytes from
    // the still-attached channel — this is the whole reason workspace
    // switches no longer corrupt alt-screen TUIs.
    return () => {
      cancelled = true;

      const containerNow = containerEl;
      const blockNewline = blockNewlineRef.current;
      if (blockNewline) {
        containerNow.removeEventListener("input", blockNewline, true);
        blockNewlineRef.current = null;
      }

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

      detachFromContainer(sid);
      entryRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // ── Re-read CSS variables when theme class/style changes ──
  useEffect(() => {
    const observer = new MutationObserver(() => {
      const entry = entryRef.current;
      if (entry) {
        entry.terminal.options.theme = buildThemeFromCSS();
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
    const entry = entryRef.current;
    if (focused && entry) {
      entry.terminal.focus();
    }
  }, [focused]);

  return (
    <div
      ref={shellRef}
      className="relative flex flex-1 w-full h-full min-w-0 min-h-0 bg-background"
    >
      <div
        ref={containerRef}
        className="block flex-1 w-full h-full min-w-0 min-h-0 overflow-hidden px-2 py-1.5 box-border [&_.codemux-terminal-wrapper]:h-full [&_.codemux-terminal-wrapper]:w-full [&_.xterm]:h-full [&_.xterm]:w-full [&_.xterm-viewport]:!bg-transparent"
      />
      <div
        ref={statusOverlayRef}
        className="terminal-overlay starting absolute inset-0 z-0 flex items-center justify-center p-4 bg-background/90"
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
