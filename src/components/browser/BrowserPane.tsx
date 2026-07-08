import { useEffect, useRef, useState, useCallback } from "react";
import { startBrowserStream, agentBrowserRun, activatePane, writeToPty } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { BrowserToolbar } from "./BrowserToolbar";
import { InspectorPanel } from "./InspectorPanel";
import { Loader2, Globe } from "lucide-react";
import type { ElementInfo } from "./inspector";
import {
  INSPECTOR_INJECT_SCRIPT,
  INSPECTOR_CLEANUP_SCRIPT,
  buildElementQueryScript,
  parseEvalResult,
  findFirstTerminalPane,
} from "./inspector";
import {
  type CdpButton,
  type ClickState,
  type ViewportInfo,
  cdpButtonFromEvent,
  heldButtonFromButtons,
  getModifiers,
  nextClickState,
  mapToViewport,
  sanitizeCursor,
  httpBaseFromStreamUrl,
  buildCursorProbeScript,
  SELECTION_SCRIPT,
  buildInsertTextScript,
  chunkString,
  PASTE_CHUNK_SIZE,
  probeDaemon,
  evalOnDaemon,
} from "./stream-protocol";

interface Props {
  browserId: string;
  focused: boolean;
  visible: boolean;
  /** When set, resolves the agent browser session by workspace instead of
   *  strictly by `browser_id`. A detached/background agent session
   *  (GUI-mode background browsing, docs/features/browser.md) has no
   *  `browser_id` until it's promoted to a pane, so the peek overlay
   *  passes the session's own `cli_session_name` as `browserId` (so the
   *  stream daemon starts against the right session) plus `workspaceId`
   *  so the `agent_browser_sessions` lookup still finds it. No-op for the
   *  normal pane-attached case, where `browser_id` alone always resolves. */
  workspaceId?: string;
  /** Suppresses the embedded address-bar toolbar. Used by the peek
   *  overlay, which renders its own compact header with a URL readout and
   *  promote/close actions instead. Defaults to showing the toolbar
   *  (today's pane behavior, unchanged). */
  hideToolbar?: boolean;
}

// Move-forwarding cadence (ms). Drags run tight for smooth text
// selection; hover is calmer; inspector mode stays at the old 10/s to
// avoid frame starvation from its in-page highlight handler.
const MOVE_INTERVAL_DRAG_MS = 16;
const MOVE_INTERVAL_HOVER_MS = 33;
const MOVE_INTERVAL_INSPECTOR_MS = 100;

// Cursor probes are an eval round-trip to the daemon — keep them well
// below the hover-move cadence.
const CURSOR_PROBE_INTERVAL_MS = 120;

// How long the screencast may stay quiet before we ask the daemon's
// HTTP status endpoint whether it is still alive. Static pages
// legitimately produce no frames, so quiet alone never reconnects.
const QUIET_BEFORE_PROBE_MS = 15000;

interface PendingMove {
  x: number;
  y: number;
  button: CdpButton;
  modifiers: number;
}

export function BrowserPane({ browserId, focused, visible, workspaceId, hideToolbar }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const viewportRef = useRef<ViewportInfo>({ width: 1280, height: 720 });
  const [status, setStatus] = useState<"starting" | "connecting" | "waiting" | "live" | "error">("starting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [hasFrame, setHasFrame] = useState(false);
  const hasFrameRef = useRef(false);
  const frameCountRef = useRef(0);
  const lastFrameTimeRef = useRef(0);

  // Input forwarding state
  const pendingMoveRef = useRef<PendingMove | null>(null);
  const moveTimerRef = useRef<number | null>(null);
  const lastMoveSentRef = useRef(0);
  const lastPointRef = useRef({ x: 0, y: 0 });
  const clickStateRef = useRef<ClickState | null>(null);
  const downButtonsRef = useRef<Set<CdpButton>>(new Set());

  // Cursor probe state
  const httpBaseRef = useRef<string | null>(null);
  const cursorBusyRef = useRef(false);
  const lastCursorAtRef = useRef(0);

  // Read initial URL from browser session state (set by ports section or other callers)
  const browserSession = useAppStore(
    (s) => s.appState?.browser_sessions.find((b) => b.browser_id === browserId),
  );
  // Check if this browser pane is backed by an agent browser session (for
  // reconnection). Pane-attached sessions resolve by `browser_id`; a
  // detached/background session (GUI-mode background browsing) has no
  // `browser_id`, so when the caller passes `workspaceId` (the peek
  // overlay), fall back to matching on that instead.
  const agentSession = useAppStore((s) => {
    const sessions = s.appState?.agent_browser_sessions;
    if (!sessions) return undefined;
    const byBrowserId = sessions.find((abs) => abs.browser_id === browserId);
    if (byBrowserId) return byBrowserId;
    if (workspaceId) {
      return sessions.find((abs) => abs.workspace_id === workspaceId);
    }
    return undefined;
  });
  const agentSessionRef = useRef(agentSession);
  agentSessionRef.current = agentSession;

  // The session ID to use for all agent-browser CLI commands.
  // When backed by an agent session, use cli_session_name so that
  // user interactions and MCP tools operate on the same Chromium session.
  const effectiveSessionId = agentSession?.cli_session_name ?? browserId;

  const [currentUrl, setCurrentUrl] = useState(
    () => agentSession?.current_url ?? browserSession?.current_url ?? "about:blank",
  );
  const currentUrlRef = useRef(currentUrl);
  currentUrlRef.current = currentUrl;

  // Sync URL display from state changes (agent navigation, browserOpenUrl).
  // Does NOT re-navigate — the agent or CLI already performed the navigation.
  // Only updates the URL bar display.
  useEffect(() => {
    const stateUrl = browserSession?.current_url;
    if (stateUrl && stateUrl !== currentUrl) {
      setCurrentUrl(stateUrl);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [browserSession?.current_url]);

  // Inspector state
  const [inspectorActive, setInspectorActive] = useState(false);
  const [selectedElement, setSelectedElement] = useState<ElementInfo | null>(null);
  const inspectorActiveRef = useRef(false);
  const injectedRef = useRef(false);
  const inspectorClickRef = useRef(false); // suppress pointerUp after inspector click

  const imgRef = useRef<HTMLImageElement | null>(null);
  const drawInfoRef = useRef({ x: 0, y: 0, w: 1280, h: 720 });
  const statusRef = useRef(status);
  statusRef.current = status;

  const sendInput = useCallback((msg: object) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }, []);

  // Inspector toggle
  const toggleInspector = useCallback(async () => {
    const next = !inspectorActiveRef.current;
    inspectorActiveRef.current = next;
    setInspectorActive(next);

    if (next) {
      setSelectedElement(null);
      try {
        await agentBrowserRun(effectiveSessionId, "eval", { script: INSPECTOR_INJECT_SCRIPT });
        injectedRef.current = true;
      } catch (err) {
        console.error("[Inspector] Injection failed:", err);
      }
    } else {
      if (injectedRef.current) {
        agentBrowserRun(effectiveSessionId, "eval", { script: INSPECTOR_CLEANUP_SCRIPT }).catch(console.error);
        injectedRef.current = false;
      }
    }
  }, [effectiveSessionId]);

  // The inspector owns the cursor while active; otherwise the cursor
  // probe drives it from the remote page's computed style.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas) canvas.style.cursor = inspectorActive ? "crosshair" : "default";
  }, [inspectorActive]);

  // Tell Agent: write selector to first terminal pane
  const handleTellAgent = useCallback(async (selector: string) => {
    const appState = useAppStore.getState().appState;
    if (!appState) return;
    const ws = appState.workspaces.find((w) => w.workspace_id === appState.active_workspace_id);
    if (!ws) return;
    const surface = ws.surfaces.find((s) => s.surface_id === ws.active_surface_id);
    if (!surface) return;
    const termPane = findFirstTerminalPane(surface.root);
    if (!termPane) return;
    await activatePane(termPane.pane_id);
    const prompt = `In the browser, select the element "${selector}" and `;
    await writeToPty(termPane.session_id, prompt);
    setSelectedElement(null);
  }, []);

  // P6 from docs/plans/browser-stream-fix.md — reactive stream URL.
  //
  // The agent browser session in app state owns the canonical
  // `stream_url`. When the Rust manager re-allocates a port (after a
  // teardown/respawn or after the bind-test rejects a stale port), the
  // backend writes the new port into `agent_browser_sessions` and the
  // store re-emits.  We read that value directly here so the WebSocket
  // reconnect is driven by state changes, not by a one-shot
  // `startBrowserStream` return value cached in a closure.
  //
  // Falling back to the value returned by `startBrowserStream` keeps
  // the legacy non-agent path (no `agentSession`) working unchanged —
  // there is no reactive source there, so the one-shot URL is fine.
  const reactiveStreamUrl = agentSession?.stream_url;

  // Start stream and connect WebSocket
  useEffect(() => {
    if (!visible) return;

    let ws: WebSocket | null = null;
    let active = true;

    (async () => {
      setStatus("starting");
      setErrorMsg(null);
      frameCountRef.current = 0;

      const streamSessionId = browserSession?.agent_session_name ?? browserId;

      let streamUrl: string;
      try {
        const startResult = await startBrowserStream(streamSessionId);
        // Prefer the reactive value when present so port re-allocations
        // don't strand the WebSocket on an old URL.  The Tauri command
        // call still happens for its side effect (spawning the daemon).
        streamUrl = reactiveStreamUrl ?? startResult;
      } catch (err) {
        if (!active) return;
        console.error("[browser] startBrowserStream FAILED", err);
        setStatus("error");
        setErrorMsg(`Failed to start browser: ${err}`);
        return;
      }

      if (!active) return;

      // The daemon serves HTTP on the same port as the stream WS —
      // used for the liveness probe, cursor probe, and clipboard bridge.
      httpBaseRef.current = httpBaseFromStreamUrl(streamUrl);

      // Auto-reconnecting WebSocket — retries until screencast is live.
      // The stream server fails with "Browser not launched" if we connect
      // before the daemon finishes launching chromium. Retry handles this.
      // After the fast retry budget is spent the pane drops to a slow
      // loop instead of giving up — the daemon can come back at any
      // time (manager respawn, host wake), and a stream pane should
      // self-heal when it does.
      let retries = 0;
      const maxRetries = 15;
      const slowRetryMs = 10000;

      function connectWS() {
        if (!active) return;
        if (retries === 0) setStatus("connecting");
        else if (retries < maxRetries) setStatus("waiting");
        // Past the fast budget: stay in "error" while slow-retrying so
        // the pill/overlay keeps showing the disconnect honestly.

        ws = new WebSocket(streamUrl);
        wsRef.current = ws;

        ws.onopen = () => {
          if (!active) return;
          setStatus("waiting");

          // Set initial viewport to match container dimensions
          const container = containerRef.current;
          if (container) {
            const rect = container.getBoundingClientRect();
            const cw = Math.round(rect.width);
            const ch = Math.round(rect.height);
            if (cw > 10 && ch > 10) {
              viewportRef.current = { width: cw, height: ch };
              agentBrowserRun(effectiveSessionId, "viewport", { width: cw, height: ch }).catch(() => {});
              sendInput({ type: "resize", width: cw, height: ch });
            }
          }
        };

        ws.onmessage = (event) => {
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === "frame") {
              frameCountRef.current++;
              lastFrameTimeRef.current = Date.now();
              if (!hasFrameRef.current) {
                hasFrameRef.current = true;
                setHasFrame(true);
              }
              if (statusRef.current !== "live") {
                setStatus("live");
                // Skip navigation on reconnect — the agent's browser is already showing the right page.
                const isReconnect = !!agentSessionRef.current;
                if (!isReconnect) {
                  // Navigate to pre-set URL when browser first goes live.
                  // Delay 300ms to let browser process fully initialize for CDP commands.
                  const targetUrl = currentUrlRef.current;
                  if (targetUrl && targetUrl !== "about:blank") {
                    setTimeout(() => {
                      agentBrowserRun(effectiveSessionId, "open", { url: targetUrl })
                        .then(() => setCurrentUrl(targetUrl))
                        .catch(console.error);
                    }, 300);
                  }
                }
              }
              retries = 0;
              const canvas = canvasRef.current;
              if (!canvas) return;
              const ctx = canvas.getContext("2d");
              if (!ctx) return;

              // Update viewport info from frame metadata
              if (msg.metadata) {
                viewportRef.current = {
                  width: msg.metadata.deviceWidth || viewportRef.current.width,
                  height: msg.metadata.deviceHeight || viewportRef.current.height,
                };
              }

              if (!imgRef.current) {
                imgRef.current = new Image();
              }
              const img = imgRef.current;
              img.onload = () => {
                const frameAspect = img.naturalWidth / img.naturalHeight;
                const canvasAspect = canvas.width / canvas.height;

                let drawW, drawH, drawX, drawY;
                if (frameAspect > canvasAspect) {
                  drawW = canvas.width;
                  drawH = canvas.width / frameAspect;
                  drawX = 0;
                  drawY = (canvas.height - drawH) / 2;
                } else {
                  drawH = canvas.height;
                  drawW = canvas.height * frameAspect;
                  drawX = (canvas.width - drawW) / 2;
                  drawY = 0;
                }

                drawInfoRef.current = { x: drawX, y: drawY, w: drawW, h: drawH };

                ctx.fillStyle = '#000';
                ctx.fillRect(0, 0, canvas.width, canvas.height);
                ctx.drawImage(img, drawX, drawY, drawW, drawH);
              };
              img.src = `data:image/jpeg;base64,${msg.data}`;
            } else if (msg.type === "status") {
              if (msg.viewportWidth && msg.viewportHeight) {
                viewportRef.current = {
                  width: msg.viewportWidth,
                  height: msg.viewportHeight,
                };
              }
            } else if (msg.type === "error") {
              const errText = msg.message || msg.error || "";
              // "Browser not launched" = daemon still starting, close to trigger retry.
              // Other errors (e.g. "Screencast already active") are benign — stay connected.
              if (errText.includes("not launched") && statusRef.current !== "live") {
                ws?.close();
              }
            }
          } catch {
            // Ignore parse errors
          }
        };

        ws.onerror = (ev) => {
          console.error("[browser] ws ERROR", ev);
        };

        ws.onclose = () => {
          if (!active) return;
          // Auto-reconnect regardless of current status — handles both
          // initial connection failures and mid-stream disconnects.
          retries++;
          if (retries < maxRetries) {
            if (statusRef.current === "live") {
              setStatus("connecting");
            }
            setTimeout(connectWS, 1500);
          } else {
            setStatus("error");
            setErrorMsg("Stream disconnected — retrying…");
            setTimeout(connectWS, slowRetryMs);
          }
        };
      }

      connectWS();
    })();

    // Liveness check. The CDP screencast only emits frames on visual
    // change, so a quiet stream is NORMAL for static pages — never a
    // reason to reconnect by itself. When the stream has been quiet for
    // a while, ask the daemon's HTTP status endpoint whether it is
    // still alive; only force a reconnect when that probe fails
    // (daemon death, stale port, half-open socket after suspend).
    let probing = false;
    const livenessInterval = setInterval(() => {
      if (!active || statusRef.current !== "live" || lastFrameTimeRef.current === 0) return;
      if (Date.now() - lastFrameTimeRef.current <= QUIET_BEFORE_PROBE_MS) return;
      if (probing) return;
      probing = true;
      const base = httpBaseRef.current;
      (base ? probeDaemon(base, 3000) : Promise.resolve(false)).then((alive) => {
        probing = false;
        if (!active || alive) return;
        if (wsRef.current?.readyState === WebSocket.OPEN) {
          console.warn("[browser] stream quiet and daemon unresponsive — reconnecting");
          wsRef.current.close();
        }
      });
    }, 5000);

    return () => {
      active = false;
      clearInterval(livenessInterval);
      // Close WebSocket on cleanup so the stream server's client count resets.
      // On StrictMode remount, the daemon is already running (*running = true),
      // so startBrowserStream returns instantly and a fresh WS connects.
      // This fresh connection triggers startScreencast with the browser launched.
      if (ws) {
        ws.close();
        wsRef.current = null;
      }
    };
    // `reactiveStreamUrl` is included so a port re-allocation in the
    // backend (after a teardown/respawn cycle) tears down this WS and
    // reconnects against the fresh URL — the bug the old deps array
    // missed (P6 in docs/plans/browser-stream-fix.md).
  }, [browserId, visible, browserSession?.agent_session_name, reactiveStreamUrl]);

  // ── Pointer input ──────────────────────────────────────────────────
  //
  // Pointer events (not mouse events) so the canvas can capture the
  // pointer during drags — releases outside the pane still produce a
  // `mouseReleased`, and selection drags keep tracking past the edge.

  const toViewport = (e: { clientX: number; clientY: number }) => {
    const canvas = canvasRef.current!;
    const rect = canvas.getBoundingClientRect();
    const point = mapToViewport(
      e,
      rect,
      { width: canvas.width, height: canvas.height },
      viewportRef.current,
      drawInfoRef.current,
    );
    lastPointRef.current = point;
    return point;
  };

  const moveInterval = () => {
    if (inspectorActiveRef.current) return MOVE_INTERVAL_INSPECTOR_MS;
    if (pendingMoveRef.current && pendingMoveRef.current.button !== "none") return MOVE_INTERVAL_DRAG_MS;
    return MOVE_INTERVAL_HOVER_MS;
  };

  const flushMove = () => {
    const m = pendingMoveRef.current;
    if (!m) return;
    pendingMoveRef.current = null;
    lastMoveSentRef.current = Date.now();
    sendInput({
      type: "input_mouse",
      eventType: "mouseMoved",
      x: m.x,
      y: m.y,
      button: m.button,
      modifiers: m.modifiers,
    });
    if (m.button === "none" && !inspectorActiveRef.current) {
      maybeProbeCursor(m.x, m.y);
    }
  };

  const scheduleMoveFlush = () => {
    if (moveTimerRef.current !== null) return;
    const wait = Math.max(0, moveInterval() - (Date.now() - lastMoveSentRef.current));
    moveTimerRef.current = window.setTimeout(() => {
      moveTimerRef.current = null;
      flushMove();
    }, wait);
  };

  const cancelPendingMove = () => {
    pendingMoveRef.current = null;
    if (moveTimerRef.current !== null) {
      clearTimeout(moveTimerRef.current);
      moveTimerRef.current = null;
    }
  };

  useEffect(() => () => cancelPendingMove(), []);

  // Reflect the remote page's cursor on the canvas (pointer over links,
  // I-beam over text, resize handles, …). Throttled eval round-trip via
  // the daemon's HTTP command relay — hover-only, never during drags.
  const maybeProbeCursor = (x: number, y: number) => {
    const base = httpBaseRef.current;
    if (!base || statusRef.current !== "live") return;
    const now = Date.now();
    if (cursorBusyRef.current || now - lastCursorAtRef.current < CURSOR_PROBE_INTERVAL_MS) return;
    cursorBusyRef.current = true;
    lastCursorAtRef.current = now;
    evalOnDaemon(base, buildCursorProbeScript(x, y), 1500)
      .then((result) => {
        const canvas = canvasRef.current;
        if (canvas && !inspectorActiveRef.current) {
          canvas.style.cursor = sanitizeCursor(result);
        }
      })
      .finally(() => {
        cursorBusyRef.current = false;
      });
  };

  const handlePointerDown = (e: React.PointerEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = toViewport(e);

    if (inspectorActiveRef.current) {
      e.preventDefault();
      e.stopPropagation();
      inspectorClickRef.current = true;
      // Query element at click coordinates
      agentBrowserRun(effectiveSessionId, "eval", { script: buildElementQueryScript(x, y) })
        .then((result) => {
          const info = parseEvalResult(result);
          if (info) setSelectedElement(info);
        })
        .catch((err) => console.error("[Inspector] Element query failed:", err))
        .finally(() => {
          // Auto-disable inspector
          inspectorActiveRef.current = false;
          setInspectorActive(false);
          if (injectedRef.current) {
            agentBrowserRun(effectiveSessionId, "eval", { script: INSPECTOR_CLEANUP_SCRIPT }).catch(console.error);
            injectedRef.current = false;
          }
        });
      return;
    }

    const button = cdpButtonFromEvent(e.button);
    if (button === "none") return;

    try {
      canvas.setPointerCapture(e.pointerId);
    } catch {
      // Pointer capture is best-effort — drags inside the pane still work.
    }

    // Land any queued hover move first so the press arrives at the
    // position the page last saw.
    flushMove();

    clickStateRef.current = nextClickState(clickStateRef.current, x, y, button, Date.now());
    downButtonsRef.current.add(button);

    sendInput({
      type: "input_mouse",
      eventType: "mousePressed",
      x,
      y,
      button,
      clickCount: clickStateRef.current.count,
      modifiers: getModifiers(e),
    });
  };

  const handlePointerUp = (e: React.PointerEvent) => {
    if (inspectorClickRef.current) {
      inspectorClickRef.current = false;
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = toViewport(e);
    const button = cdpButtonFromEvent(e.button);
    if (button === "none") return;

    // Land the final drag position before releasing so selections end
    // exactly where the pointer stopped.
    flushMove();

    downButtonsRef.current.delete(button);
    const clickCount =
      clickStateRef.current && clickStateRef.current.button === button ? clickStateRef.current.count : 1;

    sendInput({
      type: "input_mouse",
      eventType: "mouseReleased",
      x,
      y,
      button,
      clickCount,
      modifiers: getModifiers(e),
    });

    try {
      canvas.releasePointerCapture(e.pointerId);
    } catch {
      // Already released.
    }
  };

  const handlePointerCancel = () => {
    // Capture lost mid-drag (window switch, system gesture) — release
    // any held buttons remotely so the page isn't stuck dragging.
    cancelPendingMove();
    const { x, y } = lastPointRef.current;
    for (const button of downButtonsRef.current) {
      sendInput({ type: "input_mouse", eventType: "mouseReleased", x, y, button, clickCount: 1, modifiers: 0 });
    }
    downButtonsRef.current.clear();
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = toViewport(e);
    pendingMoveRef.current = {
      x,
      y,
      button: heldButtonFromButtons(e.buttons),
      modifiers: getModifiers(e),
    };
    scheduleMoveFlush();
  };

  const handlePointerLeave = (e: React.PointerEvent) => {
    // During a captured drag, leave doesn't fire until release; this is
    // the hover-exit path. Drop any queued move and return the cursor
    // to the host default.
    if (e.buttons !== 0) return;
    cancelPendingMove();
    const canvas = canvasRef.current;
    if (canvas && !inspectorActiveRef.current) canvas.style.cursor = "default";
  };

  const handleWheel = (e: React.WheelEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = toViewport(e);
    sendInput({
      type: "input_mouse",
      eventType: "mouseWheel",
      x,
      y,
      deltaX: e.deltaX,
      deltaY: e.deltaY,
      modifiers: getModifiers(e),
    });
  };

  // ── Keyboard + clipboard bridge ────────────────────────────────────
  //
  // CDP "keyDown" with text inserts the character; "rawKeyDown" for
  // non-printable keys. Do NOT send a separate "char" event — CDP
  // handles text insertion from "keyDown".
  //
  // The headless browser has its own clipboard, invisible to the host.
  // Copy/cut mirror the page selection into the host clipboard; paste
  // inserts host-clipboard text into the page's focused editable.

  const keyDownMessage = (e: React.KeyboardEvent) => {
    const isPrintable = e.key.length === 1 && !e.ctrlKey && !e.metaKey;
    return {
      type: "input_keyboard",
      eventType: isPrintable ? "keyDown" : "rawKeyDown",
      key: e.key,
      code: e.code,
      text: isPrintable ? e.key : undefined,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: getModifiers(e),
    };
  };

  const mirrorSelectionThenForward = async (msg: object) => {
    const base = httpBaseRef.current;
    if (base) {
      try {
        const sel = await evalOnDaemon(base, SELECTION_SCRIPT, 1000);
        if (typeof sel === "string" && sel.length > 0) {
          await navigator.clipboard.writeText(sel);
        }
      } catch {
        // Host clipboard unavailable — the in-page copy still happens.
      }
    }
    sendInput(msg);
  };

  const pasteFromHostClipboard = async (fallbackMsg: object) => {
    let text = "";
    try {
      text = await navigator.clipboard.readText();
    } catch {
      // Clipboard read denied/unavailable — fall through to keystroke.
    }
    const base = httpBaseRef.current;
    if (!text || !base) {
      sendInput(fallbackMsg);
      return;
    }
    // Each chunk is a daemon round-trip — bound pathological pastes.
    const MAX_PASTE_CHARS = 100_000;
    if (text.length > MAX_PASTE_CHARS) {
      console.warn(`[browser] paste truncated to ${MAX_PASTE_CHARS} characters`);
      text = text.slice(0, MAX_PASTE_CHARS);
    }
    // Chunked so each eval request fits the daemon's single-segment
    // HTTP read (see the page-scripts note in stream-protocol.ts).
    const chunks = chunkString(text, PASTE_CHUNK_SIZE);
    for (let i = 0; i < chunks.length; i++) {
      const result = await evalOnDaemon(base, buildInsertTextScript(chunks[i]), 3000);
      const inserted = result === true || result === "true";
      if (!inserted) {
        // Focus isn't an editable element (or the daemon hiccuped) —
        // forward the raw keystroke so in-page paste handlers still run.
        if (i === 0) sendInput(fallbackMsg);
        return;
      }
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Ctrl+Shift+I toggles element inspector
    if (e.ctrlKey && e.shiftKey && e.key === "I") {
      e.preventDefault();
      e.stopPropagation();
      toggleInspector();
      return;
    }
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;

    const combo = (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey;
    const key = e.key.toLowerCase();

    if (combo && (key === "c" || key === "x")) {
      e.preventDefault();
      e.stopPropagation();
      // Forwarding still performs the in-page copy/cut; the mirror puts
      // the same text on the host clipboard. For cut, the selection is
      // read before the keystroke lands.
      void mirrorSelectionThenForward(keyDownMessage(e));
      return;
    }
    if (combo && key === "v") {
      e.preventDefault();
      e.stopPropagation();
      void pasteFromHostClipboard(keyDownMessage(e));
      return;
    }

    e.preventDefault();
    e.stopPropagation();
    sendInput(keyDownMessage(e));
  };

  const handleKeyUp = (e: React.KeyboardEvent) => {
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;
    e.preventDefault();
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "v") {
      // Paste is handled via the clipboard bridge — swallow the keyUp
      // so the page doesn't see an unmatched Ctrl+V release.
      return;
    }
    sendInput({
      type: "input_keyboard",
      eventType: "keyUp",
      key: e.key,
      code: e.code,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: getModifiers(e),
    });
  };

  // ResizeObserver: sync canvas + viewport to container dimensions
  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;
    let resizeTimer: ReturnType<typeof setTimeout> | null = null;
    const observer = new ResizeObserver(() => {
      const cw = container.clientWidth;
      const ch = container.clientHeight;
      if (cw < 10 || ch < 10) return;
      // Immediately sync canvas resolution to container
      if (canvas.width !== cw || canvas.height !== ch) {
        canvas.width = cw;
        canvas.height = ch;
      }
      // Debounced: tell browser to resize viewport to match
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        viewportRef.current = { width: cw, height: ch };
        agentBrowserRun(effectiveSessionId, "viewport", { width: cw, height: ch }).catch(() => {});
        sendInput({ type: "resize", width: cw, height: ch });
      }, 200);
    });
    observer.observe(container);
    return () => {
      observer.disconnect();
      if (resizeTimer) clearTimeout(resizeTimer);
    };
  }, [browserId, sendInput]);

  useEffect(() => {
    if (focused && canvasRef.current) {
      canvasRef.current.focus();
    }
  }, [focused]);

  return (
    <div className="flex h-full w-full flex-col bg-card">
      {!hideToolbar && (
        <BrowserToolbar
          browserId={browserId}
          sessionId={effectiveSessionId}
          currentUrl={currentUrl}
          onUrlChange={setCurrentUrl}
          loading={status === "starting" || status === "connecting"}
          inspectorActive={inspectorActive}
          onInspectorToggle={toggleInspector}
        />
      )}
      {selectedElement && (
        <InspectorPanel
          element={selectedElement}
          onDismiss={() => setSelectedElement(null)}
          onTellAgent={handleTellAgent}
        />
      )}
      <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden relative">
        {/* Full-screen states only before the first frame ever arrives.
            Once content exists, reconnects keep the last frame visible
            and surface a corner pill instead of blanking the pane. */}
        {status !== "live" && !hasFrame && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-card z-10">
            {status === "error" ? (
              <>
                <Globe className="h-8 w-8 text-muted-foreground/30 mb-2" />
                <p className="text-xs text-destructive">{errorMsg || "Connection failed"}</p>
              </>
            ) : (
              <>
                <Loader2 className="h-6 w-6 animate-spin text-muted-foreground mb-2" />
                <p className="text-xs text-muted-foreground">
                  {status === "starting" && "Starting browser..."}
                  {status === "connecting" && "Connecting to stream..."}
                  {status === "waiting" && "Waiting for first frame..."}
                </p>
              </>
            )}
          </div>
        )}
        {status !== "live" && hasFrame && (
          <div
            className="absolute right-2 top-2 z-10 flex items-center gap-1.5 rounded-full border border-border bg-card/90 px-2.5 py-1 shadow-sm backdrop-blur-sm animate-in fade-in duration-200"
            style={{ animationDelay: "400ms", animationFillMode: "backwards" }}
          >
            {status === "error" ? (
              <>
                <Globe className="h-3 w-3 text-destructive" />
                <span className="text-[11px] text-destructive">{errorMsg || "Stream disconnected"}</span>
              </>
            ) : (
              <>
                <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />
                <span className="text-[11px] text-muted-foreground">Reconnecting…</span>
              </>
            )}
          </div>
        )}
        <canvas
          ref={canvasRef}
          tabIndex={0}
          className="absolute inset-0 w-full h-full outline-none"
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          onPointerMove={handlePointerMove}
          onPointerCancel={handlePointerCancel}
          onPointerLeave={handlePointerLeave}
          onContextMenu={(e) => e.preventDefault()}
          onWheel={handleWheel}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
        />
      </div>
    </div>
  );
}
