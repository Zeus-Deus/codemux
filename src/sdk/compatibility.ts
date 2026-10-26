/** Versioned renderer API. Capabilities describe compatibility, not an OS sandbox. */
export const apiVersion = 1;

export const RENDERER_PLUGIN_CAPABILITIES = Object.freeze([
  "ui.react",
  "ui.peek",
  "storage",
  "settings.read",
  "threads.read",
  "threads.send",
  "events.provider",
  "mcp",
  "process",
  "fs",
  "os",
  "workspace.read",
  "theme",
  "composer",
  "navigation",
] as const);

export type RendererPluginCapability = (typeof RENDERER_PLUGIN_CAPABILITIES)[number];

export interface RendererPluginRequirements {
  apiVersion?: number;
  capabilities?: readonly string[];
  optionalCapabilities?: readonly string[];
}

export function rendererPluginCompatibility(requirements: RendererPluginRequirements): {
  available: boolean;
  error?: string;
  capabilities: readonly RendererPluginCapability[];
} {
  if (requirements.apiVersion !== undefined && requirements.apiVersion !== apiVersion) {
    return { available: false, error: `Unsupported plugin API version ${requirements.apiVersion}; this app supports ${apiVersion}.`, capabilities: [] };
  }
  const supported = (name: string): name is RendererPluginCapability =>
    RENDERER_PLUGIN_CAPABILITIES.some((known) => known === name);
  const required = requirements.capabilities ?? [];
  const optional = requirements.optionalCapabilities ?? [];
  // Legacy JavaScript has no schema decoder. Reject malformed requirements instead
  // of accidentally activating code whose host expectations we did not understand.
  if (!Array.isArray(required) || !Array.isArray(optional) ||
      [...required, ...optional].some((name) => typeof name !== "string")) {
    return { available: false, error: "Plugin capabilities must be arrays of strings.", capabilities: [] };
  }
  const missing = required.filter((name) => !supported(name));
  if (missing.length) {
    return { available: false, error: `Unsupported plugin capabilities: ${missing.join(", ")}.`, capabilities: [] };
  }
  return {
    available: true,
    capabilities: requirements.capabilities === undefined && requirements.optionalCapabilities === undefined
      ? RENDERER_PLUGIN_CAPABILITIES
      : [...new Set([...required, ...optional].filter(supported))],
  };
}
