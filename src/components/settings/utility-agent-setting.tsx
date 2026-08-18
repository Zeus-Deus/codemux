import { Sparkles } from "lucide-react";

import { MultiProviderModelPicker } from "@/components/chat/pickers/MultiProviderModelPicker";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { LaunchReasoningPicker } from "@/components/overlays/launch-reasoning-picker";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  resolveAutoUtilitySelection,
  utilityEffortFor,
  UTILITY_PROVIDERS,
  UTILITY_SETTING_KEYS,
  type UtilityAgentMode,
} from "@/lib/utility-agent";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";
import type { AgentChatProviderKind } from "@/tauri/types";

export function UtilityAgentSetting() {
  const settings = useSettingsStore((state) => state.settings);
  const setSetting = useSettingsStore((state) => state.set);
  const capabilities = useProviderCapabilities();
  const mode = (settings[UTILITY_SETTING_KEYS.mode] ??
    "auto") as UtilityAgentMode;
  const auto = resolveAutoUtilitySelection();
  const customProvider = (settings[UTILITY_SETTING_KEYS.provider] ||
    auto?.provider ||
    "codex") as AgentChatProviderKind;
  const providerCaps = capabilities[customProvider];
  const customModel =
    settings[UTILITY_SETTING_KEYS.model] ||
    auto?.model ||
    providerCaps?.models[0]?.id ||
    null;
  const selectedModel =
    providerCaps?.models.find((entry) => entry.id === customModel) ?? null;
  const selectedEffort = settings[UTILITY_SETTING_KEYS.effort] || null;
  const reasoningOptions = (selectedModel?.effort_levels ?? []).map(
    (value) => ({
      value,
      label: providerCaps?.effort_label_map[value] ?? value,
    }),
  );

  const setMode = (next: UtilityAgentMode) => {
    setSetting(UTILITY_SETTING_KEYS.mode, next);
    if (next === "custom" && customModel) {
      setSetting(UTILITY_SETTING_KEYS.provider, customProvider);
      setSetting(UTILITY_SETTING_KEYS.model, customModel);
      setSetting(
        UTILITY_SETTING_KEYS.effort,
        utilityEffortFor(customProvider, customModel, selectedModel) ?? "",
      );
    }
  };

  return (
    <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
      <Select
        value={mode}
        onValueChange={(value) => setMode(value as UtilityAgentMode)}
      >
        <SelectTrigger className="h-9 w-[104px] bg-muted/35 text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="auto">Automatic</SelectItem>
          <SelectItem value="custom">Custom</SelectItem>
        </SelectContent>
      </Select>

      {mode === "auto" ? (
        <div
          className="flex h-9 min-w-[220px] items-center gap-2 rounded-md border border-border/70 bg-muted/25 px-3"
          data-testid="utility-agent-auto-resolution"
        >
          {auto ? (
            <>
              <ProviderLogo
                provider={auto.provider}
                className="h-4 w-4 shrink-0"
              />
              <div className="min-w-0 text-left leading-tight">
                <div className="truncate text-[11px] font-medium text-foreground">
                  {auto.model}
                </div>
                <div className="text-[9px] uppercase tracking-[0.1em] text-muted-foreground/65">
                  {auto.effort
                    ? `${auto.effort} reasoning`
                    : "fast utility pass"}
                </div>
              </div>
              <Sparkles className="ml-auto h-3.5 w-3.5 text-primary/70" />
            </>
          ) : (
            <span className="text-[11px] text-warning">
              No inexpensive model found — choose Custom
            </span>
          )}
        </div>
      ) : (
        <>
          <MultiProviderModelPicker
            // The utility runner has no Cursor backend, so offering a
            // Cursor row here would only produce
            // `utility_provider_unsupported` at generation time.
            allowedProviders={UTILITY_PROVIDERS}
            provider={customProvider}
            model={customModel}
            onProviderModelChange={(provider, model) => {
              const caps = capabilities[provider];
              const picked = caps?.models.find((entry) => entry.id === model);
              setSetting(UTILITY_SETTING_KEYS.provider, provider);
              setSetting(UTILITY_SETTING_KEYS.model, model);
              setSetting(
                UTILITY_SETTING_KEYS.effort,
                utilityEffortFor(provider, model, picked) ?? "",
              );
            }}
          />
          <LaunchReasoningPicker
            reasoningOptions={reasoningOptions}
            selectedReasoning={selectedEffort}
            defaultReasoning={selectedModel?.default_effort ?? null}
            onReasoningChange={(effort) =>
              setSetting(UTILITY_SETTING_KEYS.effort, effort)
            }
            contextOptions={[]}
            selectedContext={null}
            defaultContext={null}
            onContextChange={() => undefined}
            triggerClassName="h-9 rounded-md bg-muted/35"
          />
        </>
      )}
    </div>
  );
}
