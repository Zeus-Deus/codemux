import { useEffect, useState } from "react";
import { Workflow as WorkflowIcon } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { modelsForProvider } from "@/lib/agent-chat/capability-defaults";
import { cn } from "@/lib/utils";
import {
  WORKFLOW_PROVIDERS,
  WORKFLOW_PROVIDER_LABELS,
  WORKFLOW_SIZES,
  setWorkflowProviderPreference,
  useWorkflowSettings,
  type WorkflowAgentPolicy,
  type WorkflowProviderKind,
  type WorkflowSize,
} from "@/lib/workflows/workflow-settings";
import { useSettingsStore } from "@/stores/settings-store";
import { workflowCapabilities, type WorkflowCapability } from "@/tauri/workflows";
import { SectionHeader, SegmentedControl, SettingRow, SubsectionHeader } from "./settings-primitives";

const DEFAULT_MODEL = "__default__";
const OPENCODE_MODEL = /^[A-Za-z0-9._-]+\/\S+$/;

const POLICIES: { value: WorkflowAgentPolicy; title: string; description: string }[] = [
  {
    value: "same_provider",
    title: "Same provider as the chat",
    description: "The planner picks models from the provider you're chatting with. A Claude chat uses Claude models, a Codex chat uses Codex models. Nothing to set up.",
  },
  {
    value: "mixed",
    title: "Mix providers",
    description: "The planner can also hand subtasks to the providers turned on below, for example to get a second opinion from Codex on Claude's work.",
  },
];

function providerStatus(provider: WorkflowProviderKind, capability: WorkflowCapability | undefined, model: string): { tone: "ready" | "attention" | "off"; label: string; detail: string | null } {
  if (!capability) return { tone: "off", label: "Checking…", detail: null };
  if (!capability.live) return { tone: "off", label: "Not available", detail: capability.reason };
  if (capability.requires_model && (!model || (provider === "opencode" && !OPENCODE_MODEL.test(model)))) {
    return { tone: "attention", label: "Pick a model", detail: provider === "opencode" ? "OpenCode needs a provider/model, e.g. anthropic/claude-sonnet." : "This provider needs an explicit model." };
  }
  return { tone: "ready", label: "Ready", detail: null };
}

function ProviderRow({ provider, capability }: { provider: WorkflowProviderKind; capability: WorkflowCapability | undefined }) {
  const preference = useWorkflowSettings().providers[provider];
  const status = providerStatus(provider, capability, preference.model.trim());
  const catalog = modelsForProvider(provider).filter((model) => model.id !== "default");
  const label = WORKFLOW_PROVIDER_LABELS[provider];
  const unavailable = status.tone === "off" && !!capability;
  return <div data-settings-row data-testid={`workflow-provider-${provider}`} className={cn("grid grid-cols-[auto_minmax(0,1fr)_minmax(0,13rem)] items-center gap-x-4 gap-y-1 py-3", unavailable && "opacity-60")}>
    <Switch aria-label={`Allow ${label} in workflows`} checked={preference.enabled && !unavailable} disabled={unavailable} onCheckedChange={(enabled) => setWorkflowProviderPreference(provider, { enabled })} />
    <div className="min-w-0">
      <div className="flex items-center gap-2">
        <span className="text-body-lg font-semibold text-foreground">{label}</span>
        <span className={cn("inline-flex items-center gap-1.5 text-label", status.tone === "ready" ? "text-status-open" : status.tone === "attention" ? "text-status-working" : "text-muted-foreground")}>
          <span aria-hidden className={cn("size-1.5 rounded-full", status.tone === "ready" ? "bg-status-open" : status.tone === "attention" ? "bg-status-working" : "bg-muted-foreground/60")} />
          {status.label}
        </span>
      </div>
      {status.detail && <p className="mt-0.5 truncate text-body-sm text-muted-foreground/80" title={status.detail}>{status.detail}</p>}
    </div>
    {unavailable ? <span /> : catalog.length > 0 && !capability?.requires_model ? (
      <Select value={preference.model || DEFAULT_MODEL} onValueChange={(value) => setWorkflowProviderPreference(provider, { model: value === DEFAULT_MODEL ? "" : value })}>
        <SelectTrigger aria-label={`${label} worker model`} className="h-8 w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT_MODEL}>Default model</SelectItem>
          {catalog.map((model) => <SelectItem key={model.id} value={model.id}>{model.label}</SelectItem>)}
        </SelectContent>
      </Select>
    ) : (
      <Input aria-label={`${label} worker model`} className="h-8" placeholder={provider === "opencode" ? "provider/model" : capability?.requires_model ? "Model (required)" : "Default model"} value={preference.model} onChange={(event) => setWorkflowProviderPreference(provider, { model: event.target.value })} />
    )}
  </div>;
}

/** Which providers can run workflows on this machine, checked once per visit. */
function useWorkflowCapabilities(): { data: WorkflowCapability[] | undefined; error: unknown } {
  const [state, setState] = useState<{ data: WorkflowCapability[] | undefined; error: unknown }>({ data: undefined, error: null });
  useEffect(() => {
    let live = true;
    workflowCapabilities().then(
      (data) => { if (live) setState({ data, error: null }); },
      (error: unknown) => { if (live) setState({ data: undefined, error }); },
    );
    return () => { live = false; };
  }, []);
  return state;
}

/**
 * Settings → Workflows. Everything here is optional: `/workflow` works out of
 * the box on the chat's own provider. These choices apply to every workflow
 * started on this machine.
 */
export function WorkflowsSection() {
  const settings = useWorkflowSettings();
  const set = useSettingsStore((state) => state.set);
  const capabilities = useWorkflowCapabilities();
  const size = WORKFLOW_SIZES[settings.size];

  return <div data-testid="workflows-settings">
    <SectionHeader
      title="Workflows"
      icon={<WorkflowIcon className="size-5 text-muted-foreground" aria-hidden />}
      description={<>Split big tasks across agents working in parallel. Type <code className="rounded-sm bg-surface-2 px-1 py-px font-mono text-body-sm text-foreground">/workflow</code> and a goal in any chat: a planner splits the work, agents run it, and the report comes back to the chat. Everything here is optional.</>}
    />

    <section className="mt-2">
      <SubsectionHeader title="Which agents can be used" />
      <div role="radiogroup" aria-label="Which agents can be used" className="overflow-hidden rounded-md border border-hairline bg-surface-1">
        {POLICIES.map((policy, index) => {
          const checked = settings.agents === policy.value;
          return <label key={policy.value} data-settings-row className={cn("flex cursor-pointer items-start gap-3 px-4 py-3.5 hover:bg-surface-1", index > 0 && "border-t border-hairline")}>
            <input type="radio" name="workflow-agents" value={policy.value} checked={checked} onChange={() => set("workflows.agents", policy.value)} className="mt-1 size-4 shrink-0 accent-[var(--primary)]" />
            <span className="min-w-0">
              <span className="flex items-center gap-2 text-body-lg font-semibold text-foreground">{policy.title}{policy.value === "same_provider" && <span className="rounded-full border border-hairline-strong px-1.5 text-caption font-normal text-muted-foreground">Default</span>}</span>
              <span className="mt-1 block text-body-sm leading-relaxed text-muted-foreground/80">{policy.description}</span>
            </span>
          </label>;
        })}
      </div>
    </section>

    <section className="mt-10">
      <SubsectionHeader title="Providers" description={settings.agents === "mixed" ? "Providers the planner may use besides the chat's own. The model is a default; the planner can still pick lighter models of the chat's provider." : "Used when mixing providers and for custom scripts. Same-provider runs use the chat's provider and need no setup."} />
      <div className="divide-y divide-hairline">
        {WORKFLOW_PROVIDERS.map((provider) => <ProviderRow key={provider} provider={provider} capability={capabilities.data?.find((item) => item.provider === provider)} />)}
      </div>
      {capabilities.error != null && <p role="alert" className="mt-2 text-body-sm text-status-attention">Couldn't check which providers can run workflows. Reopen Settings to try again.</p>}
    </section>

    <section className="mt-10">
      <SubsectionHeader title="Defaults" />
      <SettingRow label="File changes" description="When edits are allowed, each agent works on its own copy. Nothing touches your checkout until you review and apply it." scope="device">
        <Select value={settings.allowWrites ? "allow" : "read_only"} onValueChange={(value) => set("workflows.file_changes", value)}>
          <SelectTrigger className="h-9 w-56"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="read_only">Read-only</SelectItem>
            <SelectItem value="allow">Allow edits, review before applying</SelectItem>
          </SelectContent>
        </Select>
      </SettingRow>
      <Separator />
      <SettingRow label="Run size" description={`Up to ${size.workers} agents, ${size.concurrency} at a time, for at most ${size.minutes} minutes.`} scope="device">
        <SegmentedControl<WorkflowSize>
          ariaLabel="Run size"
          value={settings.size}
          onChange={(value) => set("workflows.size", value)}
          options={(Object.keys(WORKFLOW_SIZES) as WorkflowSize[]).map((value) => ({ value, label: WORKFLOW_SIZES[value].label }))}
        />
      </SettingRow>
      <Separator />
      <SettingRow label="Token budget" description="Stop admitting new agents once a run has used this many tokens. Leave empty for no cap." scope="device">
        <Input type="number" min={1} inputMode="numeric" placeholder="No cap" className="h-9 w-40" value={settings.tokenBudget ?? ""} onChange={(event) => set("workflows.token_budget", event.target.value.trim())} />
      </SettingRow>
      <Separator />
      <SettingRow label="Workflow button in the composer" description="Show a shortcut to the Workflows panel next to the message box. /workflow and the + menu work either way." scope="device">
        <Switch checked={settings.composerButton} onCheckedChange={(checked) => set("workflows.composer_button", checked ? "true" : "false")} />
      </SettingRow>
    </section>
  </div>;
}
