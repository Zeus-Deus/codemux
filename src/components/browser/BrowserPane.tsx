import { useEffect, useRef, useState, useCallback, memo } from "react";
import { startBrowserStream, ensureBrowserPresetNavigation, browserPresetNavigationStatus, browserStreamBridgeEndpoints, agentBrowserRun, agentBrowserPendingConfirmation, agentBrowserRespondConfirmation, activatePane, writeToPty, type BrowserPendingConfirmation } from "@/tauri/commands";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { toast } from "@/lib/toast";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { Button } from "@/components/ui/button";
import { BrowserToolbar } from "./BrowserToolbar";
import { InspectorPanel } from "./InspectorPanel";
import { Globe, RotateCw, Copy } from "lucide-react";
import type { AgentTarget, ElementInfo } from "./inspector";
import {
  INSPECTOR_INJECT_SCRIPT,
  INSPECTOR_CLEANUP_SCRIPT,
  buildElementQueryScript,
  parseEvalResult,
  findAgentTarget,
  buildTellAgentPrompt,
} from "./inspector";
import {
  browserChromeShortcut,
  browserViewportPresetSize,
  runBrowserNav,
  type BrowserViewportPresetId,
} from "./browser-nav";
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
  remoteBrowserEndpoints,
  chunkPasteText,
  probeDaemon,
  runPaneCommand,
} from "./stream-protocol";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { markPaneReady } from "@/lib/perf/interaction-trace";
import { Spinner } from "@/components/ui/spinner";
import { isAppShortcut } from "@/lib/app-shortcuts";

interface Props {
  browserId: string;
  focused: boolean;
  visible: boolean;
  /** When set, resolves the agent browser session by workspace instead of
   *  strictly by `browser_id`. A detached/background agent session has no
   *  `browser_id` until it's promoted to a pane, so the peek overlay
   *  passes the session's own `cli_session_name` as `browserId` (so the
   *  stream daemon starts against the right session) plus `workspaceId`
   *  so the `agent_browser_sessions` lookup still finds it. No-op for the
   *  normal pane-attached case, where `browser_id` alone always resolves. */
  workspaceId?: string;
  /** Owning workspace used only for performance-trace attribution. Unlike
   *  `workspaceId`, this never changes browser-session resolution. */
  traceWorkspaceId?: string;
  /** Suppresses the embedded address-bar toolbar. Used by the peek
   *  overlay, which renders its own compact header with a URL readout and
   *  promote/close actions instead. Defaults to showing the toolbar
   *  (today's pane behavior, unchanged). */
  hideToolbar?: boolean;
  /** Pins the browser's CDP viewport to a fixed size instead of syncing
   *  it to the container. The canvas element still tracks the container's
   *  pixel size — the frame-draw letterbox scales the (larger) frame down
   *  to fit, and `mapToViewport` already maps input through the draw
   *  rect, so clicks stay accurate. Used by the peek overlay's
   *  "Desktop-size background browser" setting so the tiny popover
   *  doesn't force the agent's page to reflow at popover dimensions.
   *  Absent = today's container-sync behavior, unchanged. */
  fixedViewport?: { width: number; height: number };
  /** Reports the stream status so a host without the full-pane status
   *  states (the peek overlay's header dot) can reflect it. */
  onStatusChange?: (status: BrowserStreamStatus) => void;
}

export type BrowserStreamStatus = "starting" | "connecting" | "waiting" | "live" | "error";

type AppStoreState = ReturnType<typeof useAppStore.getState>;

/** The active workspace's agent pane, which "Send to agent" writes to. */
function resolveAgentTarget(s: AppStoreState): AgentTarget | null {
  const workspaceId = selectActiveWorkspaceId(s);
  const ws = s.appState?.workspaces.find((w) => w.workspace_id === workspaceId);
  const surface = ws?.surfaces.find((sf) => sf.surface_id === ws.active_surface_id);
  return surface ? findAgentTarget(surface) : null;
}

/** "Retrying in Ns" for the slow reconnect loop, ticking once a second. */
function RetryCountdown({ at }: { at: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  const seconds = Math.max(0, Math.ceil((at - now) / 1000));
  return <span className="tabular-nums">Retrying in {seconds}s</span>;
}

function ConfirmationCountdown({ at }: { at: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return <span className="tabular-nums">Expires in {Math.max(0, Math.ceil((at - now) / 1000))}s</span>;
}

interface PendingConfirmationState {
  sessionId: string;
  details: BrowserPendingConfirmation;
  paneEpoch: number;
}

function PendingConfirmationNotice({
  pending, busy, error, onApprove, onDeny,
}: {
  pending: PendingConfirmationState;
  busy: boolean;
  error: string | null;
  onApprove: () => void;
  onDeny: () => void;
}) {
  const canRespond = !isRemoteClient() && !!pending.details.id;
  return (
    <div className="max-w-sm space-y-2 text-center" role="alert">
      <p className="text-label font-medium text-foreground">Browser confirmation required</p>
      <p className="break-words text-label text-muted-foreground">{pending.details.description}</p>
      <p className="text-caption text-muted-foreground"><ConfirmationCountdown at={pending.details.expires_at_ms} /></p>
      {error && <p className="break-words text-label text-destructive">{error}</p>}
      {canRespond ? (
        <div className="flex items-center justify-center gap-2">
          <Button size="sm" onClick={onApprove} disabled={busy}>Approve browser action</Button>
          <Button variant="outline" size="sm" onClick={onDeny} disabled={busy}>Deny</Button>
        </div>
      ) : (
        <p className="text-label text-muted-foreground">
          {isRemoteClient() ? "Approve or deny this request on the host device." : "This request cannot be approved here. Wait for it to expire and retry."}
        </p>
      )}
    </div>
  );
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

// Slow cadence for retrying a dead stream or a failed daemon start.
const SLOW_RETRY_MS = 10000;

// A failed `startBrowserStream` may spawn a daemon each time, unlike a
// cheap WebSocket reconnect, so only a few starts retry on their own.
// After that the pane waits for "Retry now".
export const MAX_AUTO_START_RETRIES = 3;

interface PendingMove {
  x: number;
  y: number;
  button: CdpButton;
  modifiers: number;
}

// #127: memo is effective because setAppState performs structural sharing —
// PaneNode drives this with primitive props (browserId/focused/visible), so the
// pane skips re-render on backend ticks that don't touch them. Export-only
// wrapper: the component body below is unchanged.
export const BrowserPane = memo(function BrowserPane({ browserId, focused, visible, workspaceId, traceWorkspaceId, hideToolbar, fixedViewport, onStatusChange }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const urlInputRef = useRef<HTMLInputElement>(null);
  const wsRef = useRef<WebSocket | null>(null);
  const viewportRef = useRef<ViewportInfo>({ width: 1280, height: 720 });
  // A toolbar viewport preset pins the viewport the same way the
  // `fixedViewport` prop does; the prop (peek overlay) always wins.
  const [viewportPreset, setViewportPreset] = useState<BrowserViewportPresetId>("fit");
  // Ref-mirror of the effective pinned viewport so the WebSocket and
  // ResizeObserver effects can read it without listing it in their
  // dependency arrays (an inline object literal from the caller would
  // otherwise reconnect the stream every render).
  const fixedViewportRef = useRef(fixedViewport);
  fixedViewportRef.current = fixedViewport ?? browserViewportPresetSize(viewportPreset) ?? undefined;
  const [status, setStatus] = useState<BrowserStreamStatus>("starting");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingConfirmationState | null>(null);
  const pendingConfirmationRef = useRef(false);
  const visibleConfirmationRef = useRef<PendingConfirmationState | null>(null);
  const confirmationRevisionRef = useRef(0);
  const retryPresetNavigationRef = useRef(false);
  const presetErrorRef = useRef(false);
  const [pendingPreset, setPendingPreset] = useState<{ sessionId: string; paneEpoch: number } | null>(null);
  const [confirmationBusy, setConfirmationBusy] = useState(false);
  const confirmationBusyRef = useRef(false);
  const [confirmationError, setConfirmationError] = useState<string | null>(null);
  // Bumping this re-runs the stream effect from the top: "Retry now".
  const [retryNonce, setRetryNonce] = useState(0);
  // When the slow reconnect loop will try next; null while not waiting.
  const [retryAt, setRetryAt] = useState<number | null>(null);
  // Consecutive failed daemon starts; reset by a successful start or by
  // "Retry now".
  const startFailuresRef = useRef(0);
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;
  useEffect(() => {
    onStatusChangeRef.current?.(status);
  }, [status]);
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
  const appShortcutKeysRef = useRef(new Set<string>());

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
  const streamSessionId = browserSession?.agent_session_name ?? browserId;
  const paneIdentityRef = useRef({ browserId, effectiveSessionId, streamSessionId, epoch: 0 });
  if (paneIdentityRef.current.browserId !== browserId || paneIdentityRef.current.effectiveSessionId !== effectiveSessionId
      || paneIdentityRef.current.streamSessionId !== streamSessionId) {
    paneIdentityRef.current = { browserId, effectiveSessionId, streamSessionId, epoch: paneIdentityRef.current.epoch + 1 };
  }
  const paneEpoch = paneIdentityRef.current.epoch;
  const paneIsCurrent = () => paneIdentityRef.current.epoch === paneEpoch;
  const currentPendingConfirmation = pendingConfirmation?.paneEpoch === paneEpoch ? pendingConfirmation : null;

  useEffect(() => {
    retryPresetNavigationRef.current = false;
    presetErrorRef.current = false;
    pendingConfirmationRef.current = false;
    visibleConfirmationRef.current = null;
    confirmationRevisionRef.current++;
    confirmationBusyRef.current = false;
    setPendingPreset(null);
    setPendingConfirmation(null);
    setConfirmationBusy(false);
    setConfirmationError(null);
    hasFrameRef.current = false;
    lastFrameTimeRef.current = 0;
    imgRef.current = null;
    setHasFrame(false);
    if (canvasRef.current) canvasRef.current.width = canvasRef.current.width;
  }, [paneEpoch]);

  const [currentUrl, setCurrentUrl] = useState(
    () => agentSession?.current_url ?? browserSession?.current_url ?? "about:blank",
  );
  const currentUrlRef = useRef(currentUrl);
  currentUrlRef.current = currentUrl;

  useEffect(() => {
    setCurrentUrl(agentSession?.current_url ?? browserSession?.current_url ?? "about:blank");
  }, [paneEpoch]);

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

  // Only the title is subscribed: a string keeps the selector stable
  // across unrelated store ticks. The full target is re-resolved on click.
  const agentTargetTitle = useAppStore((s) => resolveAgentTarget(s)?.title ?? null);

  // Send to agent: a chat composer gets the reference appended to its
  // draft; a terminal gets it typed at the prompt (never submitted).
  const handleTellAgent = useCallback(async (element: ElementInfo) => {
    const target = resolveAgentTarget(useAppStore.getState());
    if (!target) return;
    const prompt = buildTellAgentPrompt(element, currentUrlRef.current);
    try {
      if (target.kind === "agent_chat") {
        const chat = useAgentChatStore.getState();
        const current = chat.threads[target.threadId]?.inputDraft ?? "";
        chat.setInputDraft(target.threadId, current.trim() ? `${current}\n${prompt}` : prompt);
        await activatePane(target.paneId);
      } else {
        await activatePane(target.paneId);
        await writeToPty(target.sessionId, prompt);
      }
      toast.success(`Sent to ${target.title}`);
      setSelectedElement(null);
    } catch (err) {
      toast.error("Couldn't send to the agent", { description: String(err) });
    }
  }, []);

  // A state update can trigger a reconnect, but the state URL may still be
  // stale while startBrowserStream reallocates a port. Its return value is
  // the verified listener for this attempt.
  const reactiveStreamUrl = agentSession?.stream_url;

  // Start stream and connect WebSocket
  useEffect(() => {
    if (!visible) return;

    let ws: WebSocket | null = null;
    let active = true;
    let slowRetryTimer: ReturnType<typeof setTimeout> | null = null;
    let initialNavigationTimer: ReturnType<typeof setTimeout> | null = null;
    let presetCheckScheduled = false;

    (async () => {
      pendingConfirmationRef.current = false;
      visibleConfirmationRef.current = null;
      setStatus("starting");
      setErrorMsg(null);
      setPendingConfirmation(null);
      setConfirmationError(null);
      setRetryAt(null);
      frameCountRef.current = 0;

      const remoteClient = isRemoteClient();
      const presetTargetUrl = () => useAppStore.getState().appState?.browser_sessions
        .find((session) => session.browser_id === browserId)?.current_url ?? "about:blank";

      let streamUrl: string;
      let desktopEndpoints: Awaited<ReturnType<typeof browserStreamBridgeEndpoints>> | null = null;
      try {
        streamUrl = await startBrowserStream(streamSessionId);
        if (!remoteClient) {
          desktopEndpoints = await browserStreamBridgeEndpoints(streamSessionId);
        }
      } catch (err) {
        if (!active || !paneIsCurrent()) return;
        console.error("[browser] startBrowserStream FAILED", err);
        if (String(err).includes("Browser confirmation is pending")) {
          pendingConfirmationRef.current = true;
          const presetState = await browserPresetNavigationStatus(streamSessionId).catch(() => null);
          if (!active || !paneIsCurrent()) return;
          if (presetState === "pending") setPendingPreset({ sessionId: streamSessionId, paneEpoch });
          if (!remoteClient) {
            const details = await agentBrowserPendingConfirmation(streamSessionId).catch(() => null);
            if (!active || !paneIsCurrent()) return;
            if (details) showPendingConfirmation(streamSessionId, details);
          }
          setStatus("error");
          setErrorMsg(remoteClient ? "Browser approval is waiting on the host device" : "Browser confirmation is pending");
          return;
        }
        setStatus("error");
        setErrorMsg(`Failed to start browser: ${err}`);
        // Same slow cadence as the reconnect loop: the daemon may come
        // back on its own, and the user can always retry sooner.
        startFailuresRef.current++;
        if (startFailuresRef.current <= MAX_AUTO_START_RETRIES) {
          setRetryAt(Date.now() + SLOW_RETRY_MS);
          slowRetryTimer = setTimeout(() => setRetryNonce((n) => n + 1), SLOW_RETRY_MS);
        }
        return;
      }

      startFailuresRef.current = 0;
      if (!active || !paneIsCurrent()) return;

      // The daemon serves HTTP on the same port as the stream WS —
      // used for the liveness probe, cursor probe, and clipboard bridge.
      //
      // Desktop uses an authenticated local bridge. Remote viewers use the
      // paired web-remote origin's authenticated `/proxy/browser/<port>` routes.
      let connectUrl = streamUrl;
      if (remoteClient) {
        const endpoints = remoteBrowserEndpoints(streamUrl, window.location.origin);
        if (!endpoints) {
          setStatus("error");
          setErrorMsg("Invalid browser stream endpoint");
          return;
        }
        connectUrl = endpoints.wsUrl;
        httpBaseRef.current = endpoints.httpBase;
      } else if (desktopEndpoints) {
        connectUrl = desktopEndpoints.ws_url;
        httpBaseRef.current = desktopEndpoints.http_base;
      } else {
        setStatus("error");
        setErrorMsg("Browser bridge unavailable");
        return;
      }

      // Auto-reconnecting WebSocket — retries until screencast is live.
      // The stream server fails with "Browser not launched" if we connect
      // before the daemon finishes launching chromium. Retry handles this.
      // After the fast retry budget is spent the pane drops to a slow
      // loop instead of giving up — the daemon can come back at any
      // time (manager respawn, host wake), and a stream pane should
      // self-heal when it does.
      let retries = 0;
      const maxRetries = 15;

      function scheduleReconnect(message = "Stream disconnected") {
        if (pendingConfirmationRef.current) return;
        retries++;
        if (retries < maxRetries) {
          if (statusRef.current === "live") {
            setStatus("connecting");
          }
          setTimeout(connectWS, 1500);
        } else {
          setStatus("error");
          setErrorMsg(message);
          setRetryAt(Date.now() + SLOW_RETRY_MS);
          slowRetryTimer = setTimeout(connectWS, SLOW_RETRY_MS);
        }
      }

      async function connectWS() {
        if (!active || !paneIsCurrent() || pendingConfirmationRef.current) return;
        setRetryAt(null);
        if (retries === 0) setStatus("connecting");
        else if (retries < maxRetries) setStatus("waiting");
        // Past the fast budget: stay in "error" while slow-retrying so
        // the pill/overlay keeps showing the disconnect honestly.

        // A restarted daemon can reuse its old port while the bridge rotates
        // its capability. Refresh on each scheduled reconnect so the pane
        // cannot retry a revoked URL forever. The existing retry cadence
        // bounds these IPC calls to 1.5s, then 10s.
        if (!remoteClient && retries > 0) {
          try {
            const endpoints = await browserStreamBridgeEndpoints(streamSessionId);
            if (!active || !paneIsCurrent()) return;
            connectUrl = endpoints.ws_url;
            httpBaseRef.current = endpoints.http_base;
          } catch {
            if (!active || !paneIsCurrent()) return;
            scheduleReconnect("Browser bridge unavailable");
            return;
          }
        }

        const socket = new WebSocket(connectUrl);
        ws = socket;
        wsRef.current = socket;

        socket.onopen = () => {
          if (!active || !paneIsCurrent() || wsRef.current !== socket) return;
          presetCheckScheduled = false;
          setStatus("waiting");

          // Pinned viewport: ignore container dimensions entirely — the
          // popover canvas letterboxes the larger frame down to fit.
          // Remote viewers follow the existing desktop viewport. A phone must
          // never reflow the agent’s browser when its keyboard opens.
          if (isRemoteClient()) return;
          const fixed = fixedViewportRef.current;
          if (fixed && fixed.width > 10 && fixed.height > 10) {
            viewportRef.current = { width: fixed.width, height: fixed.height };
            agentBrowserRun(effectiveSessionId, "viewport", { width: fixed.width, height: fixed.height }).catch(() => {});
            sendInput({ type: "resize", width: fixed.width, height: fixed.height });
            return;
          }

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

        socket.onmessage = (event) => {
          if (!active || !paneIsCurrent() || wsRef.current !== socket) return;
          try {
            const msg = JSON.parse(event.data);
            if (msg.type === "frame") {
              if (pendingConfirmationRef.current) return;
              frameCountRef.current++;
              lastFrameTimeRef.current = Date.now();
              if (!hasFrameRef.current) {
                hasFrameRef.current = true;
                setHasFrame(true);
                markPaneReady("browser", {
                  target:
                    traceWorkspaceId ??
                    workspaceId ??
                    selectActiveWorkspaceId(useAppStore.getState()) ??
                    undefined,
                });
              }
              if (statusRef.current !== "live" && !presetErrorRef.current) setStatus("live");
              // The backend shares one preset result across pane remounts
              // and serializes it with approval and daemon replacement.
              if (!agentSessionRef.current && !presetCheckScheduled) {
                // Navigate to pre-set URL when browser first goes live.
                // Delay 300ms to let browser process fully initialize for CDP commands.
                const targetUrl = presetTargetUrl();
                if (targetUrl && targetUrl !== "about:blank") {
                  presetCheckScheduled = true;
                  initialNavigationTimer = setTimeout(() => {
                    initialNavigationTimer = null;
                    if (!active || !paneIsCurrent()) return;
                    const latestTargetUrl = presetTargetUrl();
                    if (!latestTargetUrl || latestTargetUrl === "about:blank") {
                      presetCheckScheduled = false;
                      return;
                    }
                    const retryFailed = retryPresetNavigationRef.current;
                    retryPresetNavigationRef.current = false;
                    void ensureBrowserPresetNavigation(effectiveSessionId, latestTargetUrl, retryFailed)
                      .then(async (result) => {
                        if (!active || !paneIsCurrent()) return;
                        if (result === "completed") {
                          presetErrorRef.current = false;
                          if (!pendingConfirmationRef.current) {
                            setStatus("live");
                            setErrorMsg(null);
                          }
                        } else if (result === "pending") {
                          setPendingPreset({ sessionId: effectiveSessionId, paneEpoch });
                          if (!isRemoteClient()) {
                            const pending = await agentBrowserPendingConfirmation(effectiveSessionId).catch(() => null);
                            if (!active || !paneIsCurrent()) return;
                            if (pending) {
                              showPendingConfirmation(effectiveSessionId, pending);
                              return;
                            }
                          }
                          pendingConfirmationRef.current = true;
                          wsRef.current?.close();
                          setStatus("error");
                          setErrorMsg(isRemoteClient()
                            ? "Browser approval is waiting on the host device."
                            : "Browser confirmation is pending.");
                        } else {
                          presetErrorRef.current = true;
                          setStatus("error");
                          setErrorMsg("Preset navigation did not complete. Check the page, then retry if needed.");
                        }
                      })
                      .catch((error) => {
                        if (!active || !paneIsCurrent()) return;
                        console.error("[browser] preset navigation failed", error);
                        presetErrorRef.current = true;
                        setStatus("error");
                        setErrorMsg("Could not verify preset navigation. Check the page, then retry if needed.");
                      });
                  }, 300);
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
                if (!active || !paneIsCurrent() || wsRef.current !== socket) return;
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

                // Clear rather than paint the letterbox: the pane's themed
                // background shows through, in light and dark alike.
                ctx.clearRect(0, 0, canvas.width, canvas.height);
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
                socket.close();
              }
            }
          } catch {
            // Ignore parse errors
          }
        };

        socket.onerror = (ev) => {
          if (active && paneIsCurrent() && wsRef.current === socket) console.error("[browser] ws ERROR", ev);
        };

        socket.onclose = () => {
          if (!active || !paneIsCurrent() || wsRef.current !== socket) return;
          // Auto-reconnect regardless of current status — handles both
          // initial connection failures and mid-stream disconnects.
          scheduleReconnect();
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
      if (!active || !paneIsCurrent() || statusRef.current !== "live" || lastFrameTimeRef.current === 0) return;
      if (Date.now() - lastFrameTimeRef.current <= QUIET_BEFORE_PROBE_MS) return;
      if (probing) return;
      probing = true;
      const base = httpBaseRef.current;
      (base ? probeDaemon(base, 3000) : Promise.resolve(false)).then((alive) => {
        probing = false;
        if (!active || !paneIsCurrent() || alive) return;
        if (wsRef.current?.readyState === WebSocket.OPEN) {
          console.warn("[browser] stream quiet and daemon unresponsive — reconnecting");
          wsRef.current.close();
        }
      });
    }, 5000);

    return () => {
      active = false;
      clearInterval(livenessInterval);
      if (slowRetryTimer) clearTimeout(slowRetryTimer);
      if (initialNavigationTimer) {
        clearTimeout(initialNavigationTimer);
      }
      // Close WebSocket on cleanup so the stream server's client count resets.
      // On StrictMode remount, the daemon is already running (*running = true),
      // so startBrowserStream returns instantly and a fresh WS connects.
      // This fresh connection triggers startScreencast with the browser launched.
      if (ws) {
        ws.close();
        if (wsRef.current === ws) wsRef.current = null;
      }
    };
    // `reactiveStreamUrl` is included so a port re-allocation in the
    // backend (after a teardown/respawn cycle) tears down this WS and
    // reconnects against the fresh URL — the bug the old deps array missed.
  }, [paneEpoch, visible, reactiveStreamUrl, retryNonce]);

  const retryNow = (retryPreset = false) => {
    if (retryPreset) {
      retryPresetNavigationRef.current = true;
      presetErrorRef.current = false;
    }
    pendingConfirmationRef.current = false;
    visibleConfirmationRef.current = null;
    startFailuresRef.current = 0;
    setRetryNonce((n) => n + 1);
  };

  const showPendingConfirmation = (sessionId: string, details: BrowserPendingConfirmation) => {
    if (!paneIsCurrent()) return;
    confirmationRevisionRef.current++;
    pendingConfirmationRef.current = true;
    visibleConfirmationRef.current = { sessionId, details, paneEpoch };
    wsRef.current?.close();
    wsRef.current = null;
    httpBaseRef.current = null;
    setPendingConfirmation({ sessionId, details, paneEpoch });
    setConfirmationError(null);
    setErrorMsg(null);
    setRetryAt(null);
    setStatus("error");
  };

  useEffect(() => {
    if (!visible || isRemoteClient()) return;
    let active = true;
    let checking = false;
    const checkPending = async () => {
      if (checking || confirmationBusyRef.current) return;
      checking = true;
      const revision = confirmationRevisionRef.current;
      try {
        const pending = await agentBrowserPendingConfirmation(effectiveSessionId);
        if (!active || !paneIsCurrent() || confirmationBusyRef.current || revision !== confirmationRevisionRef.current) return;
        const shown = visibleConfirmationRef.current;
        if (pending) {
          if (shown?.sessionId !== effectiveSessionId || shown.details.id !== pending.id) {
            showPendingConfirmation(effectiveSessionId, pending);
          }
        } else if (shown?.sessionId === effectiveSessionId && !confirmationBusyRef.current) {
          confirmationRevisionRef.current++;
          setPendingPreset(null);
          presetErrorRef.current = true;
          visibleConfirmationRef.current = null;
          setPendingConfirmation(null);
          setConfirmationError(null);
          setErrorMsg("Browser confirmation is no longer pending. Retry to reconnect.");
          setStatus("error");
        }
      } catch {
        // The stream retry path reports backend failures to the pane.
      } finally {
        checking = false;
      }
    };
    void checkPending();
    const timer = setInterval(checkPending, 2000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [visible, effectiveSessionId, paneEpoch]);

  useEffect(() => {
    if (!visible || !pendingPreset || pendingPreset.paneEpoch !== paneEpoch) return;
    const sessionId = pendingPreset.sessionId;
    let active = true;
    let checking = false;
    const checkPreset = async () => {
      if (checking || confirmationBusyRef.current || !paneIsCurrent()) return;
      checking = true;
      const revision = confirmationRevisionRef.current;
      try {
        const state = await browserPresetNavigationStatus(sessionId);
        if (!active || !paneIsCurrent() || confirmationBusyRef.current || revision !== confirmationRevisionRef.current) return;
        if (state === "pending") {
          if (!isRemoteClient()) {
            const details = await agentBrowserPendingConfirmation(sessionId).catch(() => null);
            if (!active || !paneIsCurrent() || confirmationBusyRef.current || revision !== confirmationRevisionRef.current) return;
            if (details && visibleConfirmationRef.current?.details.id !== details.id) {
              showPendingConfirmation(sessionId, details);
            }
          }
        } else {
          setPendingPreset(null);
          pendingConfirmationRef.current = false;
          visibleConfirmationRef.current = null;
          setPendingConfirmation(null);
          if (state === "failed") {
            presetErrorRef.current = true;
            setStatus("error");
            setErrorMsg("Preset navigation did not complete. Check the page, then retry if needed.");
          } else {
            presetErrorRef.current = false;
            setRetryNonce((nonce) => nonce + 1);
          }
        }
      } catch {
        // Leave the request visible until the backend can report its state.
      } finally {
        checking = false;
      }
    };
    void checkPreset();
    const timer = setInterval(checkPreset, 2000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [visible, pendingPreset, paneEpoch]);

  const respondToConfirmation = async (approve: boolean) => {
    const pending = pendingConfirmation;
    if (!pending?.details.id || pending.paneEpoch !== paneEpoch || !paneIsCurrent() || confirmationBusy || isRemoteClient()) return;
    confirmationRevisionRef.current++;
    confirmationBusyRef.current = true;
    setConfirmationBusy(true);
    setConfirmationError(null);
    try {
      const followUp = await agentBrowserRespondConfirmation(pending.sessionId, pending.details.id, approve);
      if (!paneIsCurrent()) return;
      if (followUp) {
        showPendingConfirmation(pending.sessionId, followUp);
        return;
      }
      setPendingPreset(null);
      visibleConfirmationRef.current = null;
      setPendingConfirmation(null);
      if (approve) retryNow();
      else setErrorMsg("Browser action denied");
    } catch (err) {
      if (!paneIsCurrent()) return;
      const message = String(err);
      try {
        const current = await agentBrowserPendingConfirmation(pending.sessionId);
        if (!paneIsCurrent()) return;
        if (!current) {
          setPendingPreset(null);
          presetErrorRef.current = true;
          visibleConfirmationRef.current = null;
          setPendingConfirmation(null);
          setErrorMsg(message);
          setStatus("error");
          return;
        }
        if (current.id !== pending.details.id) {
          showPendingConfirmation(pending.sessionId, current);
          return;
        }
      } catch {
        // Keep the prompt visible until its backend state can be checked.
      }
      setConfirmationError(message);
    } finally {
      if (paneIsCurrent()) {
        confirmationBusyRef.current = false;
        setConfirmationBusy(false);
      }
    }
  };

  const copyErrorDetails = async () => {
    const details = [
      `Browser session: ${effectiveSessionId}`,
      `Status: ${status}`,
      `Error: ${errorMsg ?? "Connection failed"}`,
    ].join("\n");
    if (await copyToClipboard(details)) toast.success("Copied error details");
    else toast.error(COPY_FAILED_MESSAGE);
  };

  // Toolbar viewport presets. A preset pins the viewport (the frame is
  // letterboxed into the pane); "Fit pane" hands it back to the container.
  const applyViewportPreset = (preset: BrowserViewportPresetId) => {
    setViewportPreset(preset);
    const size = browserViewportPresetSize(preset);
    const container = containerRef.current;
    const target = size ?? (container
      ? { width: Math.round(container.clientWidth), height: Math.round(container.clientHeight) }
      : null);
    if (!target || target.width < 10 || target.height < 10) return;
    viewportRef.current = target;
    agentBrowserRun(effectiveSessionId, "viewport", target).catch(() => {});
    sendInput({ type: "resize", ...target });
  };

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
    runPaneCommand(base, { operation: "cursor", x, y }, 1500)
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
        const sel = await runPaneCommand(base, { operation: "selection" }, 1000);
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
    const chunks = chunkPasteText(text);
    for (let i = 0; i < chunks.length; i++) {
      const result = await runPaneCommand(base, { operation: "insert_text", text: chunks[i] }, 3000);
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
    const keyId = e.code || e.key.toLowerCase();
    appShortcutKeysRef.current.delete(keyId);
    // Ctrl+Shift+I toggles element inspector
    if (e.ctrlKey && e.shiftKey && e.key === "I") {
      e.preventDefault();
      e.stopPropagation();
      toggleInspector();
      return;
    }

    const combo = (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey;
    const key = e.key.toLowerCase();

    // Browser-chrome shortcuts the page would otherwise swallow.
    const shortcut = browserChromeShortcut(e);
    if (shortcut === "address") {
      const input = urlInputRef.current;
      if (input) {
        e.preventDefault();
        e.stopPropagation();
        input.focus();
        input.select();
        return;
      }
    } else if (shortcut) {
      e.preventDefault();
      e.stopPropagation();
      runBrowserNav(effectiveSessionId, shortcut).catch(console.error);
      return;
    }

    // Browser chrome keeps its own controls, including page reload, ahead
    // of app bindings such as the guard against reloading the interface.
    if (isAppShortcut(e.nativeEvent)) {
      appShortcutKeysRef.current.add(keyId);
      return;
    }

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
    // The action may change focus/state and modifiers may be released first.
    // Match the keydown we yielded rather than rechecking applicability.
    if (appShortcutKeysRef.current.delete(e.code || e.key.toLowerCase())) return;
    e.preventDefault();
    // handleKeyDown kept these from the page, so the page must not see a
    // release it never got a press for. Ctrl+L without a toolbar (the
    // peek) was forwarded, so its release is too.
    const shortcut = browserChromeShortcut(e);
    if (shortcut && (shortcut !== "address" || urlInputRef.current)) return;
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
      // Pinned viewport: the canvas tracks the container (above) but the
      // browser's viewport stays fixed — never re-send it or overwrite
      // viewportRef with container dims.
      if (isRemoteClient() || fixedViewportRef.current) return;
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
          onPendingConfirmation={showPendingConfirmation}
          loading={status === "starting" || status === "connecting"}
          inspectorActive={inspectorActive}
          onInspectorToggle={toggleInspector}
          urlInputRef={urlInputRef}
          viewportPreset={viewportPreset}
          // A remote viewer follows the desktop's viewport; it never sets it.
          onViewportPresetChange={fixedViewport || isRemoteClient() ? undefined : applyViewportPreset}
        />
      )}
      {selectedElement && (
        <InspectorPanel
          element={selectedElement}
          agentTargetTitle={agentTargetTitle}
          onDismiss={() => setSelectedElement(null)}
          onTellAgent={handleTellAgent}
        />
      )}
      <div ref={containerRef} className="flex-1 min-h-0 overflow-hidden relative">
        {/* Full-screen states only before the first frame ever arrives.
            Once content exists, reconnects keep the last frame visible
            and surface a corner pill instead of blanking the pane. */}
        {status !== "live" && !hasFrame && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-card z-10 px-4 text-center">
            {currentPendingConfirmation ? (
              <PendingConfirmationNotice
                pending={currentPendingConfirmation}
                busy={confirmationBusy}
                error={confirmationError}
                onApprove={() => respondToConfirmation(true)}
                onDeny={() => respondToConfirmation(false)}
              />
            ) : status === "error" ? (
              <>
                <Globe className="size-8 text-subtle-foreground mb-2" />
                <p className="max-w-sm text-label text-destructive">{errorMsg || "Connection failed"}</p>
                {retryAt !== null && (
                  <p className="mt-1 text-label text-muted-foreground">
                    <RetryCountdown at={retryAt} />
                  </p>
                )}
                <div className="mt-3 flex items-center gap-2">
                  <Button variant="outline" size="sm" onClick={() => retryNow(true)}>
                    <RotateCw className="size-3.5" />
                    Retry now
                  </Button>
                  <Button variant="ghost" size="sm" onClick={copyErrorDetails}>
                    <Copy className="size-3.5" />
                    Copy details
                  </Button>
                </div>
              </>
            ) : (
              <>
                <Spinner className="size-6 text-muted-foreground mb-2" />
                <p className="text-label text-muted-foreground">
                  {status === "starting" && "Starting browser…"}
                  {status === "connecting" && "Connecting to stream…"}
                  {status === "waiting" && "Waiting for first frame…"}
                </p>
              </>
            )}
          </div>
        )}
        {status !== "live" && hasFrame && (
          <div
            className={currentPendingConfirmation
              ? "absolute right-2 top-2 z-10 rounded-md border border-border bg-card/95 p-3 shadow-sm backdrop-blur-sm"
              : "absolute right-2 top-2 z-10 flex items-center gap-1.5 rounded-full border border-border bg-card/90 px-2.5 py-1 shadow-sm backdrop-blur-sm animate-in fade-in duration-250"}
            style={{ animationDelay: "400ms", animationFillMode: "backwards" }}
          >
            {currentPendingConfirmation ? (
              <PendingConfirmationNotice
                pending={currentPendingConfirmation}
                busy={confirmationBusy}
                error={confirmationError}
                onApprove={() => respondToConfirmation(true)}
                onDeny={() => respondToConfirmation(false)}
              />
            ) : status === "error" ? (
              <>
                <Globe className="size-3 text-destructive" />
                <span className="text-label text-destructive">{errorMsg || "Stream disconnected"}</span>
                {retryAt !== null && (
                  <span className="text-label text-muted-foreground">
                    · <RetryCountdown at={retryAt} />
                  </span>
                )}
                <button
                  type="button"
                  onClick={() => retryNow(true)}
                  className="rounded-sm text-label font-medium text-foreground underline-offset-2 hover:underline"
                >
                  Retry now
                </button>
              </>
            ) : (
              <>
                <Spinner className="size-3 text-muted-foreground" />
                <span className="text-label text-muted-foreground">Reconnecting…</span>
              </>
            )}
          </div>
        )}
        <canvas
          ref={canvasRef}
          tabIndex={0}
          aria-label="Browser page"
          // The canvas takes every key, so focus has to show on it.
          className="absolute inset-0 w-full h-full outline-none focus:outline-solid focus:outline-2 focus:-outline-offset-2 focus:outline-ring"
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
});
