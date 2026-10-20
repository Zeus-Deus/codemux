import { useEffect, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { Badge, Button, icons, Input, Switch, host, PeekChip, type Host, type PeekRenderProps } from "@codemux/sdk";
import { ComputerUseController } from "./controller";
import { INSTALL_URL, isLinuxWayland, type DoctorProbe, type DoctorReport } from "./model";

const color = (role: Parameters<typeof host.theme.cssVar>[0]) => `var(${host.theme.cssVar(role)})`;
const muted: CSSProperties = { color: color("mutedForeground"), fontSize: 13, lineHeight: 1.6 };
const row: CSSProperties = { display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" };
const section: CSSProperties = { padding: "20px 0", borderBottom: `1px solid ${color("border")}`, display: "grid", gap: 12 };

function useComputer(controller: ComputerUseController) {
  return useSyncExternalStore(controller.subscribe, controller.getState, controller.getState);
}

function Notice({ children }: { children: ReactNode }) {
  return <p role="status" style={{ ...muted, margin: 0, overflowWrap: "anywhere" }}>{children}</p>;
}

function DiagnosticIcon({ status }: { status: DoctorProbe["status"] }) {
  if (status === "ok") return <icons.CheckCircle2 className="size-4 shrink-0 text-status-open" aria-label="Passed" />;
  if (status === "warn") return <icons.TriangleAlert className="size-4 shrink-0 text-warning" aria-label="Warning" />;
  return <icons.CircleAlert className="size-4 shrink-0 text-destructive" aria-label="Error" />;
}

export function DoctorDiagnostics({ report }: { report: DoctorReport }) {
  const healthy = report.ok && report.probes.length > 0 && report.probes.every((probe) => probe.status === "ok");
  const probes = <div style={{ display: "grid", gap: 12 }}>
    {report.probes.map((probe, i) => <div key={`${probe.label}-${i}`} style={{ display: "flex", gap: 10, alignItems: "flex-start" }}>
      <span style={{ paddingTop: 2 }}><DiagnosticIcon status={probe.status} /></span>
      <div style={{ minWidth: 0 }}><span>{probe.label}</span><p style={{ ...muted, margin: "2px 0 0", overflowWrap: "anywhere" }}>{probe.message}</p>
        {probe.detail && <pre style={{ ...muted, margin: "4px 0 0", whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{probe.detail}</pre>}
      </div>
    </div>)}
  </div>;
  return <div aria-label="Driver diagnostics">
    {healthy ? <>
      <div style={{ ...row, gap: 8 }}><DiagnosticIcon status="ok" /><span>Driver ready</span></div>
      <details style={{ ...muted, marginTop: 8 }}><summary style={{ cursor: "pointer" }}>Details</summary><div style={{ paddingTop: 12 }}>{probes}</div></details>
    </> : probes}
  </div>;
}

export function ComputerSettings({ controller, peek, showWorkspace, initialCommand, onCommand, onOptIn }: {
  controller: ComputerUseController;
  peek: Pick<Host["peek"], "open">;
  showWorkspace: Host["showWorkspace"];
  initialCommand: string;
  onCommand(command: string): void;
  onOptIn(value: boolean): void;
}) {
  const state = useComputer(controller);
  const [command, setCommand] = useState(initialCommand);
  const [selected, setSelected] = useState("");
  const busy = ["detecting", "discovering", "starting", "stopping"].includes(state.phase);
  const controlling = state.phase === "active" || state.phase === "starting" || state.phase === "stopping";
  const target = state.windows.find((window) => `${window.pid}:${window.windowId}` === selected);
  return (
    <div data-computer-use="settings" style={{ maxWidth: 720, fontSize: 14 }}>
      <p style={{ ...muted, margin: "0 0 4px" }}>
        Give agents access to one application window when a structured tool is unavailable.
        Choose a window each time you enable control.
      </p>
      <section style={section} aria-label="Driver installation">
        <div style={row}><strong>cua-driver</strong>{state.version && <Badge variant="outline">{state.version}</Badge>}</div>
        <div style={{ ...row, flexWrap: "nowrap" }}>
          <Input aria-label="cua-driver executable" placeholder="cua-driver (from PATH)" value={command}
            disabled={busy || controlling} onChange={(event) => setCommand(event.target.value)} />
          <Button variant="outline" size="sm" disabled={busy || controlling || state.workspace?.local === false}
            onClick={() => { onCommand(command.trim()); void controller.detect(command); }}>
            <icons.RefreshCw />{state.phase === "detecting" ? "Checking…" : "Check installation"}
          </Button>
        </div>
        <Notice>
          Requires cua-driver 0.21.0. <a href={INSTALL_URL} target="_blank" rel="noreferrer" style={{ color: color("primary"), textDecoration: "underline" }}>Official installation guide</a>
        </Notice>
        {state.executable && <p style={{ ...muted, margin: 0, overflowWrap: "anywhere" }}>
          Executable: <code aria-label="Resolved cua-driver executable">{state.executable}</code>
        </p>}
        {state.doctor && <DoctorDiagnostics report={state.doctor} />}
      </section>
      <section style={section} aria-label="Workspace access">
        <div style={{ ...row, justifyContent: "space-between", flexWrap: "nowrap" }}>
          <div><strong>Allow in this workspace</strong><p style={{ ...muted, margin: "4px 0 0" }}>{state.workspace?.title ?? "Open a local workspace to continue."}</p></div>
          <Switch aria-label="Allow computer use in this workspace" checked={state.optedIn}
            disabled={!state.workspace?.local || state.phase === "stopping"} onCheckedChange={onOptIn} />
        </div>
        {state.workspace?.local === false && <Notice>Computer use is available only in local workspaces.</Notice>}
        {state.optedIn && state.workspace?.local && <>
          <div style={row}>
            <Button size="sm" variant="outline" disabled={busy || controlling || !state.doctor?.ok}
              onClick={() => { setSelected(""); void controller.discover(); }}>
              <icons.AppWindow />{state.phase === "discovering" ? "Finding windows…" : "Choose application window"}
            </Button>
          </div>
          {state.phase === "choosing" && <div style={{ display: "grid", gap: 10 }}>
            {state.windows.length ? <>
              <label htmlFor="computer-use-window" style={muted}>Application window</label>
              <select id="computer-use-window" value={selected} onChange={(event) => setSelected(event.target.value)}
                style={{ border: `1px solid ${color("border")}`, borderRadius: 8, padding: 10, background: color("background"), color: color("foreground"), width: "100%" }}>
                <option value="">Select a window…</option>
                {state.windows.map((window) => <option key={`${window.pid}:${window.windowId}`} value={`${window.pid}:${window.windowId}`}>
                  {window.appName ? `${window.appName} · ` : ""}{window.title || "Untitled window"} (PID {window.pid})
                </option>)}
              </select>
              <div><Button size="sm" disabled={!target} onClick={() => { if (target) void controller.enable(target); }}>Enable for this window</Button></div>
            </> : <Notice>No accessible windows were found. Open the application, then refresh the list.</Notice>}
          </div>}
          {state.target && <div style={{ ...row, justifyContent: "space-between" }}>
            <span style={{ minWidth: 0, overflowWrap: "anywhere" }}>{state.target.title || state.target.appName || "Selected window"}</span>
            <Button size="sm" variant="outline" disabled={state.phase !== "active"}
              onClick={() => { if (state.workspace && showWorkspace()) peek.open("computer-use:live", { workspaceId: state.workspace.id }); }}><icons.Monitor />Live view</Button>
          </div>}
          {state.phase === "active" && <Notice>
            Using Claude? Send a new message after enabling this window. Tools refresh between turns.
          </Notice>}
          {state.expiresAt !== null && <Notice>Control automatically stops at {new Date(state.expiresAt).toLocaleTimeString()}. Choose the window again to continue.</Notice>}
          <Notice>Only background click, type and scroll are allowed. Actions an application refuses stay blocked.
            {isLinuxWayland(state.doctor) && " Linux XWayland capture works; native Wayland capture may be unavailable."}
          </Notice>
        </>}
      </section>
      {state.error && <div role="alert" style={{ padding: "16px 0", color: "var(--destructive)", fontSize: 13, overflowWrap: "anywhere" }}>{state.error}</div>}
    </div>
  );
}

export function ComputerIndicator({ controller, peek, showWorkspace }: { controller: ComputerUseController; peek: Pick<Host["peek"], "open">; showWorkspace: Host["showWorkspace"] }) {
  const state = useComputer(controller);
  if (!state.target && state.phase !== "stopping") return null;
  const stopping = state.phase === "stopping";
  const name = state.target?.appName || state.target?.title || "window";
  const label = stopping ? "Stopping…" : state.phase === "error" ? "Computer use needs attention" : state.activeCalls ? `Controlling ${name}` : state.phase === "starting" ? "Connecting…" : `Computer · ${name}`;
  return <div data-computer-use="indicator" style={{
    display: "inline-flex", alignItems: "center", gap: 2, padding: 1, maxWidth: "min(320px, 60vw)", height: 26,
    border: `1px solid ${color("border")}`, borderRadius: 8, color: color("foreground"),
  }} onPointerDown={(event) => event.stopPropagation()}>
    <Button size="xs" variant="ghost" aria-label={`View computer use${state.target ? ` in ${state.target.title || name}` : ""}`}
      title={label} disabled={state.phase !== "active"} style={{ minWidth: 0, maxWidth: 230 }}
      onClick={() => { if (state.workspace && showWorkspace()) peek.open("computer-use:live", { workspaceId: state.workspace.id }); }}>
      <span aria-hidden style={{ width: 6, height: 6, borderRadius: "50%", flexShrink: 0 }}
        className={state.phase === "error" ? "bg-warning" : state.activeCalls ? "bg-destructive" : "bg-muted-foreground"} />
      <span aria-live="polite" aria-atomic="true" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{label}</span>
    </Button>
    <Button size="xs" variant="destructive" aria-label="Stop computer use" disabled={stopping} onClick={() => { void controller.stop(); }}>Stop</Button>
  </div>;
}

export function ComputerChatChip({ controller, workspaceId }: {
  controller: ComputerUseController; workspaceId: string | null;
}) {
  const state = useComputer(controller);
  if (!state.target || state.workspace?.id !== workspaceId) return null;
  return <PeekChip peekId="computer-use:live" label="Computer" icon={icons.Monitor}
    status={state.phase === "error" ? "error" : state.phase === "stopping" ? "stale" : state.phase === "starting" ? "connecting" : "live"} />;
}

export function ComputerPreview({ controller, workspaceId, onStatusChange }: { controller: ComputerUseController } & PeekRenderProps) {
  const state = useComputer(controller);
  const active = state.phase === "active" && state.workspace?.id === workspaceId;
  useEffect(() => {
    if (!active) return;
    void controller.refreshPreview();
    const timer = setInterval(() => { void controller.refreshPreview(); }, 750);
    return () => clearInterval(timer);
  }, [controller, active]);
  const status = !active ? "stale" : state.frame?.kind === "image" ? "live" : state.frame?.kind === "unavailable" ? "error" : "connecting";
  useEffect(() => { onStatusChange(status); }, [onStatusChange, status]);
  return <div data-computer-use="preview" style={{ width: "100%", height: "100%", display: "flex", flexDirection: "column", minHeight: 0, background: color("background") }}>
    <div style={{ ...row, flexWrap: "nowrap", justifyContent: "space-between", padding: "8px 12px", borderBottom: `1px solid ${color("border")}` }}>
      <span style={{ ...muted, overflow: "hidden", whiteSpace: "nowrap", textOverflow: "ellipsis" }}>{state.target?.title || "Selected window"}</span>
      <Button size="sm" variant="destructive" aria-label="Stop computer use in live view" disabled={!state.target || state.phase === "stopping"} onClick={() => { void controller.stop(); }}><icons.Square />Stop</Button>
    </div>
    <div style={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center", overflow: "hidden" }}>
      {active && state.frame?.kind === "image"
        ? <img src={state.frame.url} alt="Live view of the chosen application window" draggable={false} style={{ width: "100%", height: "100%", objectFit: "contain", minHeight: 0 }} />
        : <div style={{ padding: 28, textAlign: "center", maxWidth: 480 }}><icons.MonitorOff size={24} style={{ margin: "0 auto 12px", color: color("mutedForeground") }} />
          <strong>{state.phase === "expired" ? "Computer use expired" : !active ? "Computer use is stopped" : "No window image"}</strong>
          <p style={muted}>{!active ? "Choose and enable an application window in Computer use settings." : state.frame?.kind === "unavailable" ? state.frame.message : "Waiting for the driver…"}</p>
        </div>}
    </div>
  </div>;
}
