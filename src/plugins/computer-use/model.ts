export const INSTALL_URL = "https://cua.ai/docs/cua-driver/quickstart";

export interface WindowTarget {
  pid: number;
  windowId: number;
  title: string;
  appName: string;
}

export interface DoctorProbe {
  label: string;
  status: "ok" | "warn" | "error";
  message: string;
  detail?: string;
}

export interface DoctorReport {
  ok: boolean;
  probes: DoctorProbe[];
}

export function isLinuxWayland(report: DoctorReport | null): boolean {
  return report !== null &&
    report.probes.some((probe) => probe.label === "binary" && /\blinux\b/i.test(probe.message)) &&
    report.probes.some((probe) => probe.label === "display server" && /wayland/i.test(probe.message));
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function positiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function validateTarget(target: WindowTarget) {
  if (!positiveInteger(target.pid) || !positiveInteger(target.windowId)) {
    throw new Error("Choose a current application window before enabling computer use.");
  }
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

export function parseVersion(stdout: string): string | null {
  return /^cua-driver (\d+\.\d+\.\d+(?:[-+][\w.-]+)?)\s*$/m.exec(stdout)?.[1] ?? null;
}

export function parseDoctor(stdout: string): DoctorReport | null {
  const data = parseJson(stdout);
  if (!record(data) || typeof data.ok !== "boolean" || !Array.isArray(data.probes)) return null;
  const probes: DoctorProbe[] = [];
  for (const probe of data.probes) {
    if (
      !record(probe) || typeof probe.label !== "string" || typeof probe.message !== "string" ||
      (probe.status !== "ok" && probe.status !== "warn" && probe.status !== "error") ||
      (probe.detail !== undefined && typeof probe.detail !== "string")
    ) return null;
    probes.push({
      label: probe.label,
      message: probe.message,
      status: probe.status,
      ...(typeof probe.detail === "string" ? { detail: probe.detail } : {}),
    });
  }
  return { ok: data.ok, probes };
}

export function readWindows(result: unknown): WindowTarget[] {
  if (!record(result) || result.isError === true || !record(result.structuredContent)) return [];
  const windows = result.structuredContent.windows;
  if (!Array.isArray(windows)) return [];
  return windows.flatMap((window): WindowTarget[] => {
    if (
      !record(window) || !positiveInteger(window.pid) || !positiveInteger(window.window_id) ||
      typeof window.title !== "string"
    ) return [];
    return [{
      pid: window.pid,
      windowId: window.window_id,
      title: window.title,
      appName: typeof window.app_name === "string" ? window.app_name : "",
    }];
  });
}

export const OBSERVATION_TOOLS = ["get_window_state"] as const;
export const INPUT_TOOLS = ["click", "type_text", "scroll"] as const;
export const CONTROL_TOOLS = [...OBSERVATION_TOOLS, ...INPUT_TOOLS] as const;
export const DISCOVERY_TOOLS = ["list_windows", "get_screen_size"] as const;
export const CONTROL_LIFETIME_MS = 10 * 60 * 1000;

export function discoveryFiles(): { policy: string; manifest: string } {
  const tools = DISCOVERY_TOOLS;
  return {
    policy: JSON.stringify({ allow: { tools } }),
    manifest: JSON.stringify({
      version: 3, expires_after: "10m", idle_timeout: "10m",
      allow: { tools }, resources: { desktop: { display: true } },
    }),
  };
}

export function viewFiles(target: WindowTarget): { policy: string; manifest: string } {
  validateTarget(target);
  const manifest = {
    version: 3, expires_after: "10m", idle_timeout: "10m",
    allow: { tools: ["get_window_state"] },
    resources: { desktop: { windows: [{ pid: target.pid, window_id: target.windowId }], display: false } },
  };
  return { policy: JSON.stringify({ allow: { tools: ["get_window_state"] } }), manifest: JSON.stringify(manifest) };
}

export function controlInstructions(target: WindowTarget): string {
  return `Prefer structured tools (browser tools, app-specific plugin tools) before computer use. Only target pid ${target.pid}, window_id ${target.windowId}. ` +
    'Input requires pid, window_id and delivery_mode:"background"; click also requires modifier:[]. ' +
    'Use fresh element tokens. Do not retry refused actions in the foreground or capture the full desktop.';
}

export function controlFiles(target: WindowTarget): { policy: string; manifest: string } {
  validateTarget(target);
  // The policy filters tools/list; the manifest separately limits resources.
  const policy = {
    allow: {
      tools: OBSERVATION_TOOLS,
      rules: INPUT_TOOLS.map((tool) => ({
        tool,
        constraints: {
          pid: { allowed: [target.pid] },
          window_id: { allowed: [target.windowId] },
          delivery_mode: { allowed: ["background"] },
          ...(tool === "click" ? { modifier: { allowed: [[]] } } : {}),
        },
      })),
    },
  };
  const manifest = {
    version: 3,
    expires_after: "10m",
    idle_timeout: "10m",
    allow: { tools: [...OBSERVATION_TOOLS, ...INPUT_TOOLS] },
    resources: {
      desktop: {
        applications: [target.pid],
        windows: [{ pid: target.pid, window_id: target.windowId }],
        display: false,
      },
    },
  };
  return { policy: JSON.stringify(policy), manifest: JSON.stringify(manifest) };
}

export function toolText(result: unknown): string | null {
  if (!record(result) || !Array.isArray(result.content)) return null;
  const messages = result.content.flatMap((block): string[] =>
    record(block) && block.type === "text" && typeof block.text === "string" ? [block.text] : [],
  );
  return messages.length ? messages.join("\n") : null;
}

export type PreviewFrame = { kind: "image"; url: string } | { kind: "unavailable"; message: string };

export function readFrame(result: unknown): PreviewFrame {
  if (!record(result)) return { kind: "unavailable", message: "The driver returned no image." };
  const state = record(result.structuredContent) ? result.structuredContent : null;
  const captureError = state && record(state.screenshot_error) ? state.screenshot_error : null;
  if (captureError || state?.screenshot_frame_valid === false || result.isError === true) {
    const reason = captureError?.reason;
    return {
      kind: "unavailable",
      message: typeof reason === "string" ? reason : toolText(result) ?? "The driver could not capture this window.",
    };
  }
  if (Array.isArray(result.content)) {
    for (const block of result.content) {
      if (
        record(block) && block.type === "image" && block.mimeType === "image/png" &&
        typeof block.data === "string" && block.data.length > 0 &&
        /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(block.data)
      ) return { kind: "image", url: `data:image/png;base64,${block.data}` };
    }
  }
  return { kind: "unavailable", message: "The driver returned no window image." };
}

export function connectionFailure(result: unknown): string | null {
  if (!record(result) || result.isError !== true) return null;
  return `cua-driver could not connect to the desktop: ${toolText(result) ?? "unknown connection error"}`;
}
