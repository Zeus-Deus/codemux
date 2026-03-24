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
  const [currentUrl, setCurrentUrl] = useState("about:blank");
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

          // Set initial viewport to match container dimensions
          const container = containerRef.current;
          if (container) {
            const rect = container.getBoundingClientRect();
            const cw = Math.round(rect.width);
            const ch = Math.round(rect.height);
            if (cw > 10 && ch > 10) {
              viewportRef.current = { width: cw, height: ch };
              agentBrowserRun(browserId, "viewport", { width: cw, height: ch }).catch(() => {});
              sendInput({ type: "resize", width: cw, height: ch });
            }
          }
        };

        ws.onmessage = (event) => {
          try {
            const msg = JSON.parse(event.data);

            if (msg.type === "frame") {
              if (statusRef.current !== "live") setStatus("live");
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

              const container = containerRef.current;
              console.log('BROWSER DEBUG frame:', {
                frameW: msg.metadata?.deviceWidth, frameH: msg.metadata?.deviceHeight,
                canvasAttrW: canvas.width, canvasAttrH: canvas.height,
                canvasCssW: canvas.clientWidth, canvasCssH: canvas.clientHeight,
                canvasStyleW: canvas.style.width, canvasStyleH: canvas.style.height,
                containerW: container?.clientWidth, containerH: container?.clientHeight,
              });

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
    const { x, y } = mapCoordinates(e, canvas, viewportRef.current, drawInfoRef.current);
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
    if (e.buttons === 0) return;
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
      console.log('BROWSER DEBUG resize:', {
        containerW: cw, containerH: ch,
        canvasAttrW: canvas.width, canvasAttrH: canvas.height,
        canvasCssW: canvas.clientWidth, canvasCssH: canvas.clientHeight,
      });
      // Debounced: tell browser to resize viewport to match
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        viewportRef.current = { width: cw, height: ch };
        agentBrowserRun(browserId, "viewport", { width: cw, height: ch }).catch(() => {});
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
