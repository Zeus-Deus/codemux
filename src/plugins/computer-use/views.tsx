import { useEffect, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { Badge, Button, icons, Input, Switch, host } from "@codemux/sdk";
import type { PeekAdapter, PeekBodyProps } from "./adapter";
import { ComputerUseController } from "./controller";
import { INSTALL_URL } from "./model";

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

export function ComputerSettings({ controller, peek, initialCommand, onCommand, onOptIn }: {
  controller: ComputerUseController;
  peek: PeekAdapter;
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
        {state.doctor && <div style={{ display: "grid", gap: 10 }} aria-label="Driver diagnostics">
          {state.doctor.probes.map((probe, i) => (
            <div key={`${probe.label}-${i}`} style={{ display: "flex", gap: 10, alignItems: "baseline" }}>
              <Badge variant="outline">{probe.status}</Badge>
              <div><span>{probe.label}</span><p style={{ ...muted, margin: "2px 0 0" }}>{probe.message}</p>
                {probe.detail && <details style={muted}><summary>Details</summary><pre style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{probe.detail}</pre></details>}
              </div>
            </div>
          ))}
        </div>}
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
              onClick={() => { if (state.workspace) peek.open(state.workspace.id); }}><icons.Monitor />Live view</Button>
          </div>}
          <Notice>Only background click, type and scroll are allowed. Actions an application refuses stay blocked. Linux XWayland capture works; native Wayland capture may be unavailable.</Notice>
        </>}
      </section>
      {state.error && <div role="alert" style={{ padding: "16px 0", color: "var(--destructive)", fontSize: 13, overflowWrap: "anywhere" }}>{state.error}</div>}
    </div>
  );
}

export function ComputerIndicator({ controller, peek }: { controller: ComputerUseController; peek: PeekAdapter }) {
  const state = useComputer(controller);
  if (!state.target && state.phase !== "stopping") return null;
  const stopping = state.phase === "stopping";
  const label = stopping ? "Stopping computer use…" : state.phase === "error" ? "Computer use needs attention" : state.activeCalls ? "Agent is controlling the computer" : state.phase === "starting" ? "Connecting computer use…" : "Computer use enabled";
  return <div data-computer-use="indicator" role="status" style={{
    position: "fixed", bottom: 20, left: "50%", transform: "translateX(-50%)", zIndex: 100,
    display: "flex", alignItems: "center", gap: 12, padding: "8px 10px 8px 14px", maxWidth: "calc(100vw - 32px)",
    border: `1px solid ${color("border")}`, borderRadius: 12, background: color("background"), color: color("foreground"),
    boxShadow: "0 6px 24px #0003", pointerEvents: "auto",
  }}>
    <icons.Monitor size={16} style={{ flexShrink: 0 }} />
    <div style={{ minWidth: 0, fontSize: 12 }}><strong>{label}</strong>
      {state.target && <div style={{ ...muted, fontSize: 11, maxWidth: 260, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{state.target.title || state.target.appName}</div>}
    </div>
    <Button size="icon-sm" variant="ghost" aria-label="Open computer-use live view" disabled={state.phase !== "active"}
      onClick={() => { if (state.workspace) peek.open(state.workspace.id); }}><icons.Eye /></Button>
    <Button size="sm" variant="destructive" disabled={stopping} onClick={() => { void controller.stop(); }}><icons.Square />Stop</Button>
  </div>;
}

export function ComputerPreview({ controller, workspaceId, onStatusChange }: { controller: ComputerUseController } & PeekBodyProps) {
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
      <Button size="sm" variant="destructive" disabled={!active} onClick={() => { void controller.stop(); }}><icons.Square />Stop</Button>
    </div>
    <div style={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center", overflow: "hidden" }}>
      {active && state.frame?.kind === "image"
        ? <img src={state.frame.url} alt="Live view of the chosen application window" draggable={false} style={{ width: "100%", height: "100%", objectFit: "contain", minHeight: 0 }} />
        : <div style={{ padding: 28, textAlign: "center", maxWidth: 480 }}><icons.MonitorOff size={24} style={{ margin: "0 auto 12px", color: color("mutedForeground") }} />
          <strong>{!active ? "Computer use is stopped" : "No window image"}</strong>
          <p style={muted}>{!active ? "Choose and enable an application window in Computer use settings." : state.frame?.kind === "unavailable" ? state.frame.message : "Waiting for the driver…"}</p>
        </div>}
    </div>
  </div>;
}

export function UnavailableSettings() {
  return <Notice>Computer use requires a desktop build with plugin process, file and peek support.</Notice>;
}
