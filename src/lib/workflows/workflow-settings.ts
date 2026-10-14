import { useMemo } from "react";
import { useSettingsStore, SETTINGS_DEFAULTS } from "@/stores/settings-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { DEFAULT_WORKFLOW_LIMITS, type WorkflowCapability, type WorkflowLimits, type WorkflowRoute } from "@/tauri/workflows";

/** Providers the workflow engine can route to, in Settings order. */
export const WORKFLOW_PROVIDERS = ["claude", "codex", "opencode", "cursor", "hermes", "grok"] as const;
export type WorkflowProviderKind = (typeof WORKFLOW_PROVIDERS)[number];

export const WORKFLOW_PROVIDER_LABELS: Record<WorkflowProviderKind, string> = {
  claude: "Claude",
  codex: "Codex",
  opencode: "OpenCode",
  cursor: "Cursor",
  hermes: "Hermes",
  grok: "Grok",
};

export type WorkflowAgentPolicy = "same_provider" | "mixed";
export type WorkflowSize = "small" | "standard" | "large";

export interface WorkflowProviderPreference {
  enabled: boolean;
  /** Default worker model; empty means the provider's own default. */
  model: string;
}

export interface WorkflowSettings {
  agents: WorkflowAgentPolicy;
  providers: Record<WorkflowProviderKind, WorkflowProviderPreference>;
  allowWrites: boolean;
  size: WorkflowSize;
  tokenBudget: number | null;
  composerButton: boolean;
}

/** Run size presets. `workers` caps how many subtasks a planned workflow may
 *  create; `concurrency` caps how many run at once. */
export const WORKFLOW_SIZES: Record<WorkflowSize, { label: string; workers: number; concurrency: number; minutes: number }> = {
  small: { label: "Small", workers: 3, concurrency: 2, minutes: 15 },
  standard: { label: "Standard", workers: 5, concurrency: 4, minutes: 30 },
  large: { label: "Large", workers: 10, concurrency: 8, minutes: 60 },
};

const DEFAULT_ENABLED: ReadonlySet<WorkflowProviderKind> = new Set(["claude", "codex"]);

export function isWorkflowProvider(provider: string): provider is WorkflowProviderKind {
  return (WORKFLOW_PROVIDERS as readonly string[]).includes(provider);
}

function parseProviders(raw: string): WorkflowSettings["providers"] {
  let stored: Record<string, Partial<WorkflowProviderPreference>> = {};
  try {
    const parsed: unknown = raw ? JSON.parse(raw) : {};
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) stored = parsed as typeof stored;
  } catch {
    // A corrupt value falls back to the defaults rather than blocking workflows.
  }
  return Object.fromEntries(WORKFLOW_PROVIDERS.map((provider) => {
    const entry = stored[provider];
    return [provider, {
      enabled: typeof entry?.enabled === "boolean" ? entry.enabled : DEFAULT_ENABLED.has(provider),
      model: typeof entry?.model === "string" ? entry.model : "",
    }];
  })) as WorkflowSettings["providers"];
}

export function parseWorkflowSettings(settings: Record<string, string>): WorkflowSettings {
  const read = (key: string) => settings[key] ?? SETTINGS_DEFAULTS[key] ?? "";
  const size = read("workflows.size");
  const budget = Number(read("workflows.token_budget"));
  return {
    agents: read("workflows.agents") === "mixed" ? "mixed" : "same_provider",
    providers: parseProviders(read("workflows.providers")),
    allowWrites: read("workflows.file_changes") === "allow",
    size: size === "small" || size === "large" ? size : "standard",
    tokenBudget: Number.isFinite(budget) && budget > 0 ? Math.floor(budget) : null,
    composerButton: read("workflows.composer_button") === "true",
  };
}

export function useWorkflowSettings(): WorkflowSettings {
  const settings = useSettingsStore((state) => state.settings);
  return useMemo(() => parseWorkflowSettings(settings), [settings]);
}

export function getWorkflowSettings(): WorkflowSettings {
  return parseWorkflowSettings(useSettingsStore.getState().settings);
}

export function setWorkflowProviderPreference(provider: WorkflowProviderKind, patch: Partial<WorkflowProviderPreference>) {
  const current = getWorkflowSettings().providers;
  const next = { ...current, [provider]: { ...current[provider], ...patch } };
  useSettingsStore.getState().set("workflows.providers", JSON.stringify(next));
}

export function workflowLimits(settings: WorkflowSettings): WorkflowLimits {
  const size = WORKFLOW_SIZES[settings.size];
  return {
    ...DEFAULT_WORKFLOW_LIMITS,
    concurrency: size.concurrency,
    wall_time_ms: size.minutes * 60_000,
    token_budget: settings.tokenBudget,
  };
}

export interface WorkflowModelOption {
  id: string;
  label: string;
}

export interface WorkflowChatContext {
  provider: AgentChatProviderKind;
  model: string | null;
}

export interface WorkflowRoutePlan {
  routes: WorkflowRoute[];
  /** Human labels for each route id, shown to the planner and in the UI. */
  labels: Record<string, string>;
  /** Why no workflow can start, phrased for the user. */
  problem: string | null;
}

const OPENCODE_MODEL = /^[A-Za-z0-9._-]+\/\S+$/;
/** Extra same-provider models offered to the planner beside the chat's own. */
const SAME_PROVIDER_EXTRA_MODELS = 2;

export function workflowRouteId(provider: string, model: string | null | undefined): string {
  const raw = model ? `${provider}-${model}` : provider;
  return raw.replace(/[^A-Za-z0-9_.:-]+/g, "-").replace(/-+/g, "-").replace(/^-|-$/g, "").slice(0, 80) || provider;
}

function usableModel(provider: WorkflowProviderKind, capability: WorkflowCapability | undefined, model: string | null): boolean {
  if (!capability?.requires_model) return true;
  if (!model) return false;
  return provider !== "opencode" || OPENCODE_MODEL.test(model);
}

function unavailableReason(provider: WorkflowProviderKind, capability: WorkflowCapability | undefined): string {
  const label = WORKFLOW_PROVIDER_LABELS[provider];
  if (!capability) return `Checking whether ${label} can run workflows. Try again in a moment.`;
  return capability.reason ? `${label} can't run workflows here: ${capability.reason}` : `${label} can't run workflows yet.`;
}

/**
 * Choose the provider/model routes for a run.
 *
 * With a chat context and the default "same provider" policy, the run stays on
 * the chat's provider: the chat's model plus a couple of the provider's other
 * models, so the planner can give narrow subtasks to lighter models without
 * any setup. "Mix providers" (and runs started outside a chat) add every
 * provider enabled in Settings that can run live.
 */
export function buildWorkflowRoutes({
  settings,
  capabilities,
  chat,
  models,
  modelLabel,
}: {
  settings: WorkflowSettings;
  capabilities: readonly WorkflowCapability[];
  chat: WorkflowChatContext | null;
  /** The provider's model catalog, flagship first. */
  models: (provider: WorkflowProviderKind) => readonly WorkflowModelOption[];
  modelLabel?: (provider: WorkflowProviderKind, model: string) => string;
}): WorkflowRoutePlan {
  const routes: WorkflowRoute[] = [];
  const labels: Record<string, string> = {};
  const capability = (provider: WorkflowProviderKind) => capabilities.find((item) => item.provider === provider);
  const label = (provider: WorkflowProviderKind, model: string | null) => {
    const name = WORKFLOW_PROVIDER_LABELS[provider];
    if (!model) return `${name} (default model)`;
    const shown = modelLabel?.(provider, model) ?? models(provider).find((item) => item.id === model)?.label ?? model;
    return shown.toLowerCase().startsWith(name.toLowerCase()) ? shown : `${name} ${shown}`;
  };
  const add = (provider: WorkflowProviderKind, model: string | null) => {
    const id = workflowRouteId(provider, model);
    if (routes.some((route) => route.id === id)) return;
    routes.push({ id, provider, model, effort: null });
    labels[id] = label(provider, model);
  };

  if (chat) {
    if (!isWorkflowProvider(chat.provider)) {
      return { routes, labels, problem: "Workflows can't run on this chat's agent. Switch to Claude or Codex, or choose providers in Settings → Workflows." };
    }
    const provider = chat.provider;
    const info = capability(provider);
    if (!info?.live) return { routes, labels, problem: unavailableReason(provider, info) };
    const model = chat.model && chat.model !== "default" ? chat.model : null;
    if (!usableModel(provider, info, model)) {
      return { routes, labels, problem: `Choose a model for ${WORKFLOW_PROVIDER_LABELS[provider]} in the chat before starting a workflow.` };
    }
    add(provider, model);
    if (settings.agents === "same_provider") {
      const extras = models(provider)
        .map((item) => item.id)
        .filter((id) => id !== "default" && id !== model && usableModel(provider, info, id))
        .slice(0, SAME_PROVIDER_EXTRA_MODELS);
      for (const id of extras) add(provider, id);
      return { routes, labels, problem: null };
    }
  }

  for (const provider of WORKFLOW_PROVIDERS) {
    const preference = settings.providers[provider];
    if (!preference.enabled || chat?.provider === provider) continue;
    const info = capability(provider);
    const model = preference.model.trim() || null;
    if (!info?.live || !usableModel(provider, info, model)) continue;
    add(provider, model);
  }
  if (routes.length === 0) {
    return { routes, labels, problem: "No workflow provider is ready. Turn one on in Settings → Workflows." };
  }
  return { routes, labels, problem: null };
}

/** A short run name from the user's goal. */
export function workflowTitleFromGoal(goal: string): string {
  const line = goal.trim().split(/\r?\n/)[0]?.replace(/\s+/g, " ").trim() ?? "";
  if (!line) return "Workflow";
  return line.length > 72 ? `${line.slice(0, 71).trimEnd()}…` : line;
}

/** `/workflow <goal>` → the goal, or null when the text is not a workflow command. */
export function parseWorkflowCommand(text: string): { goal: string } | null {
  const match = /^\/workflow(?:\s+([\s\S]*))?$/.exec(text.trim());
  if (!match) return null;
  return { goal: (match[1] ?? "").trim() };
}
