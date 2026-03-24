import { useEffect, useRef, useState, useCallback } from "react";
import { startBrowserStream } from "@/tauri/commands";
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
  return { x: Math.round(x), y: Math.round(y) };
}

export function BrowserPane({ browserId, focused, visible }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const viewportRef = useRef<ViewportInfo>({ width: 1280, height: 720 });
  const [status, setStatus] = useState<"starting" | "connecting" | "waiting" | "live" | "error">("starting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [currentUrl, setCurrentUrl] = useState("about:blank");
  const imgRef = useRef<HTMLImageElement | null>(null);

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
    let cancelled = false;

    (async () => {
      setStatus("starting");
      setErrorMsg(null);

      let streamUrl: string;
      try {
        console.log("[BrowserPane] Calling startBrowserStream...");
        streamUrl = await startBrowserStream(browserId);
        console.log("[BrowserPane] Got stream URL:", streamUrl);
      } catch (err) {
        console.error("[BrowserPane] startBrowserStream failed:", err);
        if (cancelled) return;
        setStatus("error");
        setErrorMsg(`Failed to start browser: ${err}`);
        return;
      }

      if (cancelled) return;
      setStatus("connecting");
      console.log("[BrowserPane] Connecting WebSocket to", streamUrl);

      ws = new WebSocket(streamUrl);
      wsRef.current = ws;

      ws.onopen = () => {
        console.log("[BrowserPane] WebSocket OPEN");
        if (cancelled) return;
        setStatus("waiting");
      };

      ws.onmessage = (event) => {
        if (cancelled) return;
        try {
          const msg = JSON.parse(event.data);
          console.log("[BrowserPane] Message:", msg.type, msg.type === "frame" ? `(${msg.data?.length} bytes)` : JSON.stringify(msg));

          if (msg.type === "frame") {
            setStatus("live");
            const canvas = canvasRef.current;
            if (!canvas) return;
            const ctx = canvas.getContext("2d");
            if (!ctx) return;

            // Decode base64 JPEG and draw on canvas
            if (!imgRef.current) {
              imgRef.current = new Image();
            }
            const img = imgRef.current;
            img.onload = () => {
              if (canvas.width !== img.width || canvas.height !== img.height) {
                canvas.width = img.width;
                canvas.height = img.height;
              }
              ctx.drawImage(img, 0, 0);
            };
            img.src = `data:image/jpeg;base64,${msg.data}`;

            // Update viewport from metadata
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
            console.error("[BrowserPane] Server error:", msg.message);
            setErrorMsg(msg.message);
          }
        } catch (parseErr) {
          console.error("[BrowserPane] Parse error:", parseErr);
        }
      };

      ws.onerror = (err) => {
        console.error("[BrowserPane] WebSocket error:", err);
        if (cancelled) return;
        setStatus("error");
        setErrorMsg("WebSocket connection failed");
      };

      ws.onclose = (ev) => {
        console.log("[BrowserPane] WebSocket CLOSE code:", ev.code, "reason:", ev.reason);
        if (cancelled) return;
        setStatus("error");
        setErrorMsg("Stream disconnected");
      };
    })();

    return () => {
      cancelled = true;
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
    // Only send move events if button is pressed (dragging)
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

  // Keyboard handlers
  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Don't capture tab management shortcuts
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;

    e.preventDefault();
    e.stopPropagation();

    sendInput({
      type: "input_keyboard",
      eventType: "keyDown",
      key: e.key,
      code: e.code,
      modifiers: getModifiers(e),
    });

    // For printable characters, also send a char event
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
      sendInput({
        type: "input_keyboard",
        eventType: "char",
        text: e.key,
        modifiers: getModifiers(e),
      });
    }
  };

  const handleKeyUp = (e: React.KeyboardEvent) => {
    if (e.ctrlKey && (e.key === "t" || e.key === "w" || e.key === "k")) return;

    e.preventDefault();
    sendInput({
      type: "input_keyboard",
      eventType: "keyUp",
      key: e.key,
      code: e.code,
      modifiers: getModifiers(e),
    });
  };

  // Focus canvas when pane is focused
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
      <div className="flex-1 min-h-0 overflow-hidden relative">
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
          className="w-full h-full object-contain outline-none cursor-default"
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
