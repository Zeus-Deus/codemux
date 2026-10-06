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
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  AUTOMATIC_NEW_CHAT_PROVIDER,
  NEW_CHAT_DEFAULT_PROVIDERS,
  NEW_CHAT_SETTING_KEYS,
  resolveNewChatModelDefaults,
} from "@/lib/agent-chat/new-chat-defaults";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useSettingsStore } from "@/stores/settings-store";

/**
 * Settings → Agent → New chats → Default model.
 *
 * "Automatic" stores nothing and keeps the built-in choice (Claude on its
 * default model). "Custom" stores provider, model and effort; every write
 * also re-seeds untouched composer drafts so the open composer agrees with
 * the setting straight away.
 */
export function NewChatModelSetting() {
  const settings = useSettingsStore((state) => state.settings);
  const setSetting = useSettingsStore((state) => state.set);
  const capabilities = useProviderCapabilities();
  const custom = !!settings[NEW_CHAT_SETTING_KEYS.provider];
  // Read through the resolver so the controls show what a new chat will
  // actually get, including the fallbacks for a model that went away.
  const resolved = resolveNewChatModelDefaults();
  const providerCaps = selectCapabilities(capabilities, resolved.provider);
  const model = selectModel(providerCaps, resolved.model);
  const reasoningOptions = (model?.effort_levels ?? []).map((value) => ({
    value,
    label: providerCaps?.effort_label_map[value] ?? value,
  }));

  const write = (
    provider: string,
    nextModel: string,
    effort: string | null,
  ) => {
    setSetting(NEW_CHAT_SETTING_KEYS.provider, provider);
    setSetting(NEW_CHAT_SETTING_KEYS.model, nextModel);
    setSetting(NEW_CHAT_SETTING_KEYS.effort, effort ?? "");
    useChatDraftStore.getState().applyNewChatDefaults();
  };

  const setMode = (next: string) => {
    if (next === "custom") {
      write(resolved.provider, resolved.model, resolved.effort);
    } else {
      write("", "", null);
    }
  };

  return (
    <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
      <Select value={custom ? "custom" : "auto"} onValueChange={setMode}>
        <SelectTrigger
          className="h-9 w-[104px] bg-muted/35 text-label"
          aria-label="Default model mode"
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="auto">Automatic</SelectItem>
          <SelectItem value="custom">Custom</SelectItem>
        </SelectContent>
      </Select>

      {custom ? (
        <>
          <MultiProviderModelPicker
            allowedProviders={NEW_CHAT_DEFAULT_PROVIDERS}
            provider={resolved.provider}
            model={resolved.model}
            onProviderModelChange={(provider, nextModel) =>
              write(provider, nextModel, null)
            }
          />
          <LaunchReasoningPicker
            reasoningOptions={reasoningOptions}
            selectedReasoning={resolved.effort}
            defaultReasoning={model?.default_effort ?? null}
            onReasoningChange={(effort) =>
              write(resolved.provider, resolved.model, effort)
            }
            contextOptions={[]}
            selectedContext={null}
            defaultContext={null}
            onContextChange={() => undefined}
            triggerClassName="h-9 rounded-md bg-muted/35"
          />
        </>
      ) : (
        <div
          className="flex h-9 min-w-[200px] items-center gap-2 rounded-md border border-border/70 bg-muted/25 px-3"
          data-testid="new-chat-model-auto-resolution"
        >
          <ProviderLogo
            provider={AUTOMATIC_NEW_CHAT_PROVIDER}
            className="size-4 shrink-0"
          />
          <div className="min-w-0 text-left leading-tight">
            <div className="truncate text-label font-medium text-foreground">
              {model?.label ?? resolved.model}
            </div>
            <Eyebrow>Claude default</Eyebrow>
          </div>
        </div>
      )}
    </div>
  );
}
