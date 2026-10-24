import { useEffect, useState, useSyncExternalStore, type CSSProperties, type ReactNode } from "react";
import { Badge, Button, buttonVariants, icons, Input, Spinner, Switch, host, PeekChip, type Host, type PeekRenderProps, type PluginSetupStatus } from "@codemux/sdk";
import { ComputerUseController, DRIVER_VERSION, type ComputerUseState, type DriverState, type InstallStage } from "./controller";
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
      <div style={{ ...row, gap: 8 }}><DiagnosticIcon status="ok" /><span>All diagnostics passed</span></div>
      <details style={{ ...muted, marginTop: 8 }}><summary style={{ cursor: "pointer" }}>Details</summary><div style={{ paddingTop: 12 }}>{probes}</div></details>
    </> : probes}
  </div>;
}

const SOURCE_LABEL: Record<NonNullable<DriverState["source"]>, string> = {
  system: "Found on this computer",
  managed: "Installed by Codemux",
  custom: "Custom path",
};

/** One-line setup state for the plugin's Extensions row. */
export function setupStatus(state: Pick<ComputerUseState, "driver" | "phase">): PluginSetupStatus {
  if (state.phase === "installing") return { label: "Installing driver…", tone: "neutral" };
  switch (state.driver.status) {
    case "unchecked": return { label: "Checking driver…", tone: "neutral" };
    case "ready": return { label: "Ready to use", tone: "ready" };
    case "missing": return { label: "Setup needed · driver not installed", tone: "attention" };
    case "incompatible": return { label: "Setup needed · compatible driver required", tone: "attention" };
    case "needs-setup": return { label: "Setup needed · finish driver setup", tone: "attention" };
    case "error": return { label: "Setup needed · driver check failed", tone: "attention" };
  }
}

function driverHeadline(state: ComputerUseState): { title: string; detail: string | null } {
  if (state.phase === "installing") return { title: "Installing the driver…", detail: null };
  if (state.phase === "detecting" || state.driver.status === "unchecked") return { title: "Checking for the driver…", detail: null };
  const { status, message, source } = state.driver;
  if (status === "ready") return { title: "Driver ready", detail: source === "managed" ? "Installed and checked by Codemux." : source === "custom" ? "Your custom path passed the version and diagnostic checks." : "A compatible installation was found automatically." };
  if (status === "missing") return { title: "Driver not installed", detail: message ?? (state.managed?.supported === false
    ? `${state.managed.unsupportedReason ?? "Codemux can’t install the driver here."} Install it with the official guide, then check again.`
    : "Install the driver to let agents work in a window you choose.") };
  if (status === "incompatible") return { title: "Compatible driver needed", detail: message };
  if (status === "needs-setup") return { title: "Driver setup needed", detail: message };
  return { title: "Couldn’t check the driver", detail: message };
}

const INSTALL_STEPS = ["Download", "Verify", "Check"] as const;

function InstallProgress({ install }: { install: InstallStage }) {
  const step = install.stage === "downloading" ? 0 : install.stage === "verifying" ? 1 : 2;
  const percent = install.stage === "downloading" && install.total > 0 ? Math.min(100, Math.round((install.received / install.total) * 100)) : null;
  const mb = (bytes: number) => (bytes / 1_000_000).toFixed(1);
  const label = install.stage === "downloading"
    ? install.total > 0 ? `Downloading… ${mb(install.received)} of ${mb(install.total)} MB` : "Downloading…"
    : install.stage === "verifying" ? "Verifying the download…" : "Checking the driver…";
  return <div aria-label="Installation progress" style={{ display: "grid", gap: 8 }}>
    <ol style={{ ...row, gap: 14, listStyle: "none", margin: 0, padding: 0 }}>
      {INSTALL_STEPS.map((name, i) => <li key={name} aria-current={i === step ? "step" : undefined}
        style={{ ...row, gap: 6, fontSize: 13, color: color(i <= step ? "foreground" : "mutedForeground") }}>
        {i < step ? <icons.CheckCircle2 className="size-3.5 text-status-open" aria-label="Done" />
          : i === step ? <icons.CircleDot className="size-3.5" aria-hidden /> : <icons.Circle className="size-3.5" aria-hidden />}
        {name}
      </li>)}
    </ol>
    <div role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}
      style={{ height: 4, borderRadius: 2, background: color("border"), overflow: "hidden" }}>
      <div style={{ height: "100%", width: `${percent ?? (step === 0 ? 0 : 100)}%`, background: color("primary"), transition: "width 150ms linear" }} />
    </div>
    <Notice>{label}</Notice>
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
  const [saved, setSaved] = useState(initialCommand);
  const [selected, setSelected] = useState("");
  // Check the driver whenever setup opens; refresh() leaves active control alone.
  useEffect(() => { void controller.refresh(initialCommand); }, [controller, initialCommand]);
  const busy = ["detecting", "installing", "discovering", "starting", "stopping"].includes(state.phase);
  const controlling = state.phase === "active" || state.phase === "starting" || state.phase === "stopping";
  const target = state.windows.find((window) => `${window.pid}:${window.windowId}` === selected);
  const { driver, managed } = state;
  const ready = driver.status === "ready";
  const headline = driverHeadline(state);
  const checking = state.phase === "detecting" || (driver.status === "unchecked" && state.phase === "idle");
  const canInstall = (driver.status === "missing" || driver.status === "incompatible") && managed?.supported === true;
  const unsupported = (driver.status === "missing" || driver.status === "incompatible") && managed?.supported === false;
  // Any problem the doctor reports stays in view, even on a ready driver;
  // only an all-clear report folds away under Advanced.
  const doctorNeedsAttention = !!state.doctor && (driver.status === "needs-setup" || !state.doctor.ok
    || state.doctor.probes.some((probe) => probe.status !== "ok"));
  const applyCommand = (next: string) => {
    const value = next.trim();
    setCommand(value);
    setSaved(value);
    onCommand(value);
    void controller.detect(value);
  };
  const install = () => {
    // A managed install replaces a custom path; the user's own binary stays.
    if (saved) { setCommand(""); setSaved(""); onCommand(""); }
    void controller.install();
  };
  const recheck = <Button size="sm" variant="outline" disabled={busy || controlling} onClick={() => { void controller.detect(saved); }}>
    <icons.RefreshCw />Check again
  </Button>;
  return (
    <div data-computer-use="settings" style={{ maxWidth: 720, fontSize: 14 }}>
      <p style={{ ...muted, margin: "0 0 4px" }}>
        Give agents access to one application window when a structured tool is unavailable.
        Choose a window each time you enable control.
      </p>
      <section style={section} aria-label="Driver installation">
        <div style={{ ...row, justifyContent: "space-between" }}>
          <div style={{ ...row, flexWrap: "nowrap", minWidth: 0 }}>
            <icons.Cpu className="size-4 shrink-0" aria-hidden style={{ color: color("mutedForeground") }} />
            <div style={{ minWidth: 0 }}>
              <strong>Computer use driver</strong>
              <p style={{ ...muted, margin: 0 }}>cua-driver · {state.version === DRIVER_VERSION ? state.version : `requires ${DRIVER_VERSION}`}</p>
            </div>
          </div>
          {ready && driver.source && <Badge variant="outline">{SOURCE_LABEL[driver.source]}</Badge>}
        </div>
        <div role="status" aria-live="polite" style={{ display: "grid", gap: 4 }}>
          <div style={{ ...row, gap: 8 }}>
            {busy && !controlling ? <Spinner className="size-4" /> : ready ? <DiagnosticIcon status="ok" />
              : driver.status === "error" ? <DiagnosticIcon status="error" /> : <DiagnosticIcon status="warn" />}
            <span data-driver-status={driver.status}>{headline.title}</span>
          </div>
          {headline.detail && !checking && state.phase !== "installing" && <p style={{ ...muted, margin: 0, overflowWrap: "anywhere" }}>{headline.detail}</p>}
        </div>
        {state.install && <InstallProgress install={state.install} />}
        {state.installError && state.phase !== "installing" && <p role="alert" style={{ ...muted, margin: 0, color: "var(--destructive)", overflowWrap: "anywhere" }}>
          {state.installError}
        </p>}
        {driver.status === "needs-setup" && <Notice>
          Some steps happen in your operating system’s settings, such as allowing screen recording or accessibility access.
          Follow the items below, then check again.
        </Notice>}
        {doctorNeedsAttention && state.doctor && <DoctorDiagnostics report={state.doctor} />}
        {state.phase !== "installing" && !checking && <div style={row}>
          {canInstall && <Button size="sm" disabled={busy || controlling} onClick={install}>
            <icons.Download />{state.installError ? "Retry installation" : driver.status === "incompatible" ? "Install compatible driver" : "Install driver"}
          </Button>}
          {unsupported && <a href={INSTALL_URL} target="_blank" rel="noreferrer" className={buttonVariants({ size: "sm" })}>
            <icons.ExternalLink />Open installation guide
          </a>}
          {(!canInstall || driver.status === "incompatible") && recheck}
          {canInstall && driver.status === "incompatible" && <span style={muted}>Your existing installation stays in place.</span>}
        </div>}
        {canInstall && state.phase !== "installing" && <Notice>
          Downloads cua-driver {DRIVER_VERSION} from its official release, verifies the checksum and keeps it in Codemux’s data folder.
          It doesn’t change PATH or other installations.
        </Notice>}
        {unsupported && driver.status === "incompatible" && managed?.unsupportedReason && <Notice>
          {managed.unsupportedReason} Install {DRIVER_VERSION} with the official guide, then check again.
        </Notice>}
        <details style={{ ...muted }}>
          <summary style={{ cursor: "pointer" }}>Advanced · custom path and diagnostics</summary>
          <div style={{ display: "grid", gap: 10, paddingTop: 12 }}>
            {state.executable && <p style={{ ...muted, margin: 0, overflowWrap: "anywhere" }}>
              Executable: <code aria-label="Resolved cua-driver executable">{state.executable}</code>
            </p>}
            <label htmlFor="computer-use-executable" style={muted}>Driver executable path</label>
            <div style={{ ...row, flexWrap: "nowrap" }}>
              <Input id="computer-use-executable" aria-label="cua-driver executable" placeholder="Detect automatically"
                value={command} spellCheck={false} disabled={busy || controlling} onChange={(event) => setCommand(event.target.value)} />
              <Button variant="outline" size="sm" disabled={busy || controlling || !command.trim()} onClick={() => applyCommand(command)}>
                Use this path
              </Button>
            </div>
            <p style={{ ...muted, margin: 0 }}>
              Leave empty to detect automatically. A custom path must pass the same version and diagnostic checks.
            </p>
            {saved && <div><Button variant="ghost" size="sm" disabled={busy || controlling} onClick={() => applyCommand("")}>
              <icons.RotateCcw />Use automatic detection
            </Button></div>}
            {state.doctor && !doctorNeedsAttention && <DoctorDiagnostics report={state.doctor} />}
            <p style={{ ...muted, margin: 0 }}>
              Manual installation: <a href={INSTALL_URL} target="_blank" rel="noreferrer" style={{ color: color("primary"), textDecoration: "underline" }}>official installation guide</a>
            </p>
          </div>
        </details>
      </section>
      <section style={section} aria-label="Workspace access">
        <div style={{ ...row, justifyContent: "space-between", flexWrap: "nowrap" }}>
          <div><strong>Allow in this workspace</strong><p style={{ ...muted, margin: "4px 0 0" }}>{state.workspace?.title ?? "Open a local workspace to continue."}</p></div>
          <Switch aria-label="Allow computer use in this workspace" checked={state.optedIn}
            disabled={!state.workspace?.local || state.phase === "stopping" || (!state.optedIn && !ready)} onCheckedChange={onOptIn} />
        </div>
        {state.workspace?.local === false && <Notice>Computer use is available only in local workspaces.</Notice>}
        {state.workspace?.local && !ready && !checking && state.phase !== "installing" && <Notice>Finish driver setup to choose a window.</Notice>}
        {state.optedIn && state.workspace?.local && <>
          <div style={row}>
            <Button size="sm" variant="outline" disabled={busy || controlling || !ready || !state.doctor?.ok}
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

/** Whether the chat chip draws anything for this workspace: only while a
 *  window is chosen there. Shared with the status area's visibility data. */
export function computerChipVisible(state: ComputerUseState, workspaceId: string | null): boolean {
  return !!state.target && state.workspace?.id === workspaceId;
}

export function ComputerChatChip({ controller, workspaceId }: {
  controller: ComputerUseController; workspaceId: string | null;
}) {
  const state = useComputer(controller);
  if (!computerChipVisible(state, workspaceId)) return null;
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
