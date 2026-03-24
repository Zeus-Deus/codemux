import { useEffect, useRef, useState, useCallback } from "react";
import { startBrowserStream, agentBrowserRun } from "@/tauri/commands";
import { BrowserToolbar } from "./BrowserToolbar";
import { Loader2, Globe } from "lucide-react";

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
): { x: number; y: number } {
  const rect = canvas.getBoundingClientRect();
  const x = ((e.clientX - rect.left) / rect.width) * viewport.width;
  const y = ((e.clientY - rect.top) / rect.height) * viewport.height;
  return { x: Math.max(0, Math.round(x)), y: Math.max(0, Math.round(y)) };
}

export function BrowserPane({ browserId, focused, visible }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const viewportRef = useRef<ViewportInfo>({ width: 1280, height: 720 });
  const [status, setStatus] = useState<"starting" | "connecting" | "waiting" | "live" | "error">("starting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [currentUrl, setCurrentUrl] = useState("about:blank");
  const imgRef = useRef<HTMLImageElement | null>(null);
  const statusRef = useRef(status);
  statusRef.current = status;

  const sendInput = useCallback((msg: object) => {
    const ws = wsRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(msg));
    }
  }, []);

  // Start stream and connect WebSocket
  useEffect(() => {
    if (!visible) return;

    let ws: WebSocket | null = null;
    let active = true;

    (async () => {
      setStatus("starting");
      setErrorMsg(null);

      let streamUrl: string;
      try {
        streamUrl = await startBrowserStream(browserId);
      } catch (err) {
        if (!active) return;
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
        };

        ws.onmessage = (event) => {
          try {
            const msg = JSON.parse(event.data);

            if (msg.type === "frame") {
              if (statusRef.current !== "live") setStatus("live");
              retries = 0;
              const canvas = canvasRef.current;
              const container = containerRef.current;
              if (!canvas || !container) return;
              const ctx = canvas.getContext("2d");
              if (!ctx) return;

              // Size canvas to container
              const rect = container.getBoundingClientRect();
              const cw = Math.round(rect.width);
              const ch = Math.round(rect.height);
              if (canvas.width !== cw || canvas.height !== ch) {
                canvas.width = cw;
                canvas.height = ch;
              }

              if (!imgRef.current) {
                imgRef.current = new Image();
              }
              const img = imgRef.current;
              img.onload = () => {
                // Draw frame scaled to fill canvas — viewport should match pane size
                ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
              };
              img.src = `data:image/jpeg;base64,${msg.data}`;

              if (msg.metadata) {
                viewportRef.current = {
                  width: msg.metadata.deviceWidth || 1280,
                  height: msg.metadata.deviceHeight || 720,
                };
              }
            } else if (msg.type === "status") {
              if (msg.viewportWidth && msg.viewportHeight) {
                viewportRef.current = {
                  width: msg.viewportWidth,
                  height: msg.viewportHeight,
                };
              }
            } else if (msg.type === "error") {
              // "Browser not launched" = daemon still starting, will auto-retry via onclose
              if (statusRef.current !== "live") {
                ws?.close();
              }
            }
          } catch {
            // Ignore parse errors
          }
        };

        ws.onerror = () => {};

        ws.onclose = () => {
          if (!active) return;
          // Auto-reconnect if we haven't received frames yet
          if (statusRef.current !== "live" && retries < maxRetries) {
            retries++;
            setTimeout(connectWS, 1500);
          } else if (statusRef.current !== "live") {
            setStatus("error");
            setErrorMsg("Failed to connect to browser stream");
          }
        };
      }

      connectWS();
    })();

    return () => {
      active = false;
      // Close WebSocket on cleanup so the stream server's client count resets.
      // On StrictMode remount, the daemon is already running (*running = true),
      // so startBrowserStream returns instantly and a fresh WS connects.
      // This fresh connection triggers startScreencast with the browser launched.
      if (ws) {
        ws.close();
        wsRef.current = null;
      }
    };
  }, [browserId, visible]);

  // Mouse handlers
  const handleMouseDown = (e: React.MouseEvent) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current);
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
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current);
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
    if (e.buttons === 0) return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current);
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
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current);
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

  // ResizeObserver: update canvas + browser viewport when pane resizes
  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    if (!container || !canvas) return;
    let resizeTimer: ReturnType<typeof setTimeout> | null = null;
    const observer = new ResizeObserver(() => {
      const rect = container.getBoundingClientRect();
      const cw = Math.round(rect.width);
      const ch = Math.round(rect.height);
      if (cw < 10 || ch < 10) return;
      if (canvas.width !== cw || canvas.height !== ch) {
        canvas.width = cw;
        canvas.height = ch;
      }
      // Debounced viewport resize — tell browser to match pane size
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        viewportRef.current = { width: cw, height: ch };
        agentBrowserRun(browserId, "viewport", { width: cw, height: ch }).catch(() => {});
      }, 300);
    });
    observer.observe(container);
    return () => {
      observer.disconnect();
      if (resizeTimer) clearTimeout(resizeTimer);
    };
  }, [browserId]);

  useEffect(() => {
    if (focused && canvasRef.current) {
      canvasRef.current.focus();
    }
  }, [focused]);

  return (
    <div className="flex h-full w-full flex-col bg-card">
      <BrowserToolbar
        browserId={browserId}
        currentUrl={currentUrl}
        onUrlChange={setCurrentUrl}
        loading={status === "starting" || status === "connecting"}
      />
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
          className="absolute inset-0 w-full h-full outline-none cursor-default"
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
