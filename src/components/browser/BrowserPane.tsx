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

interface Props {
  browserId: string;
  focused: boolean;
  visible: boolean;
}

interface ViewportInfo {
  width: number;
  height: number;
}

function getModifiers(e: React.MouseEvent | React.KeyboardEvent): number {
  let m = 0;
  if (e.shiftKey) m |= 1;
  if (e.ctrlKey) m |= 2;
  if (e.altKey) m |= 4;
  if (e.metaKey) m |= 8;
  return m;
}

function mapCoordinates(
  e: React.MouseEvent,
  canvas: HTMLCanvasElement,
  viewport: ViewportInfo,
  drawInfo: { x: number; y: number; w: number; h: number },
): { x: number; y: number } {
  const rect = canvas.getBoundingClientRect();
  // Convert CSS pixel position to canvas pixel position
  const scaleX = canvas.width / rect.width;
  const scaleY = canvas.height / rect.height;
  const canvasX = (e.clientX - rect.left) * scaleX;
  const canvasY = (e.clientY - rect.top) * scaleY;
  // Map from draw area to viewport coordinates
  const x = ((canvasX - drawInfo.x) / drawInfo.w) * viewport.width;
  const y = ((canvasY - drawInfo.y) / drawInfo.h) * viewport.height;
  return { x: Math.max(0, Math.round(x)), y: Math.max(0, Math.round(y)) };
}

export function BrowserPane({ browserId, focused, visible }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const viewportRef = useRef<ViewportInfo>({ width: 1280, height: 720 });
  const [status, setStatus] = useState<"starting" | "connecting" | "waiting" | "live" | "error">("starting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const frameCountRef = useRef(0);
  const lastFrameTimeRef = useRef(0);

  // Read initial URL from browser session state (set by ports section or other callers)
  const browserSession = useAppStore(
    (s) => s.appState?.browser_sessions.find((b) => b.browser_id === browserId),
  );
  // Check if this browser pane is backed by an agent browser session (for reconnection)
  const agentSession = useAppStore(
    (s) => s.appState?.agent_browser_sessions?.find((abs) => abs.browser_id === browserId),
  );
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
  const inspectorClickRef = useRef(false); // suppress mouseUp after inspector click

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
  }, [browserId]);

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
        streamUrl = await startBrowserStream(streamSessionId);
      } catch (err) {
        if (!active) return;
        console.error("[browser] startBrowserStream FAILED", err);
        setStatus("error");
        setErrorMsg(`Failed to start browser: ${err}`);
        return;
      }

      if (!active) return;

      // Auto-reconnecting WebSocket — retries until screencast is live.
      // The stream server fails with "Browser not launched" if we connect
      // before the daemon finishes launching chromium. Retry handles this.
      let retries = 0;
      const maxRetries = 15;

      function connectWS() {
        if (!active) return;
        setStatus(retries === 0 ? "connecting" : "waiting");

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
          if (retries < maxRetries) {
            retries++;
            if (statusRef.current === "live") {
              setStatus("connecting");
            }
            setTimeout(connectWS, 1500);
          } else {
            setStatus("error");
            setErrorMsg("Failed to connect to browser stream");
          }
        };
      }

      connectWS();
    })();

    // Frame liveness check: if no frame arrives for 5s while live,
    // close the WebSocket to trigger reconnection via onclose handler.
    const livenessInterval = setInterval(() => {
      if (!active) return;
      if (statusRef.current === "live" && lastFrameTimeRef.current > 0) {
        const stale = Date.now() - lastFrameTimeRef.current > 5000;
        if (stale && wsRef.current?.readyState === WebSocket.OPEN) {
          console.warn("[browser] Frame timeout — reconnecting");
          wsRef.current.close();
        }
      }
    }, 2000);

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
  }, [browserId, visible, browserSession?.agent_session_name]);

  // Mouse handlers
  const handleMouseDown = (e: React.MouseEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current, drawInfoRef.current);

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

    sendInput({
      type: "input_mouse",
      eventType: "mousePressed",
      x,
      y,
      button: "left",
      clickCount: 1,
      modifiers: getModifiers(e),
    });
  };

  const handleMouseUp = (e: React.MouseEvent) => {
    if (inspectorClickRef.current) {
      inspectorClickRef.current = false;
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current, drawInfoRef.current);
    sendInput({
      type: "input_mouse",
      eventType: "mouseReleased",
      x,
      y,
      button: "left",
      modifiers: getModifiers(e),
    });
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (!inspectorActiveRef.current && e.buttons === 0) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current, drawInfoRef.current);
    sendInput({
      type: "input_mouse",
      eventType: "mouseMoved",
      x,
      y,
      button: "none",
      modifiers: getModifiers(e),
    });
  };

  const handleWheel = (e: React.WheelEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current, drawInfoRef.current);
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

  // Keyboard handlers — CDP "keyDown" with text inserts the character.
  // "rawKeyDown" for non-printable keys (Backspace, Enter, etc.).
  // Do NOT send a separate "char" event — CDP handles text insertion from "keyDown".
  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Ctrl+Shift+I toggles element inspector
    if (e.ctrlKey && e.shiftKey && e.key === "I") {
      e.preventDefault();
      e.stopPropagation();
      toggleInspector();
      return;
    }
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;
    e.preventDefault();
    e.stopPropagation();

    const isPrintable = e.key.length === 1 && !e.ctrlKey && !e.metaKey;

    sendInput({
      type: "input_keyboard",
      eventType: isPrintable ? "keyDown" : "rawKeyDown",
      key: e.key,
      code: e.code,
      text: isPrintable ? e.key : undefined,
      windowsVirtualKeyCode: e.keyCode,
      modifiers: getModifiers(e),
    });
  };

  const handleKeyUp = (e: React.KeyboardEvent) => {
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;
    e.preventDefault();
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
      <BrowserToolbar
        browserId={browserId}
        sessionId={effectiveSessionId}
        currentUrl={currentUrl}
        onUrlChange={setCurrentUrl}
        loading={status === "starting" || status === "connecting"}
        inspectorActive={inspectorActive}
        onInspectorToggle={toggleInspector}
      />
      {selectedElement && (
        <InspectorPanel
          element={selectedElement}
          onDismiss={() => setSelectedElement(null)}
          onTellAgent={handleTellAgent}
        />
      )}
      <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden relative">
        {status !== "live" && (
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
        <canvas
          ref={canvasRef}
          tabIndex={0}
          className={`absolute inset-0 w-full h-full outline-none ${inspectorActive ? "cursor-crosshair" : "cursor-default"}`}
          onMouseDown={handleMouseDown}
          onMouseUp={handleMouseUp}
          onMouseMove={handleMouseMove}
          onWheel={handleWheel}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
        />
      </div>
    </div>
  );
}
