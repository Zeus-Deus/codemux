import { ArrowRight } from "lucide-react";

import {
  endpointLabel,
  handoffLabel,
  modelLabel,
} from "@/lib/agent-chat/provider-switch";
import type { ProviderSwitchItem } from "@/lib/agent-chat/types";
import {
  selectCapabilities,
  selectModel,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import type { AgentChatProviderKind } from "@/tauri/types";

import { ProviderLogo } from "./provider-logo";

function useModelLabel(provider: AgentChatProviderKind, model: string | null) {
  const label = useProviderCapabilities(
    (state) => selectModel(selectCapabilities(state, provider), model)?.label,
  );
  return modelLabel(model, label);
}

function Endpoint({ provider, label }: { provider: AgentChatProviderKind; label: string }) {
  return (
    <span className="inline-flex min-w-0 items-center gap-1.5">
      <ProviderLogo provider={provider} className="size-3 shrink-0 opacity-80" />
      <span className="truncate">{label}</span>
    </span>
  );
}

/**
 * Where a chat moved to another provider in place: a quiet hairline
 * divider, "Claude · Opus 4.8 → Codex · GPT-5.5 · context carried over".
 * Everything above it reached the new provider as a handoff.
 */
export function ProviderSwitchDivider({ item }: { item: ProviderSwitchItem }) {
  const from = endpointLabel(item.fromProvider, useModelLabel(item.fromProvider, item.fromModel));
  const to = endpointLabel(item.toProvider, useModelLabel(item.toProvider, item.toModel));
  const carried = handoffLabel(item.handoffKind, item.truncated);
  return (
    <div
      data-testid="provider-switch-divider"
      role="separator"
      aria-label={`Switched from ${from} to ${to}, ${carried}`}
      className="flex w-full items-center gap-3 py-1 text-muted-foreground/70"
    >
      <span className="h-px flex-1 bg-border/60" />
      <span className="inline-flex min-w-0 items-center gap-1.5 font-mono text-label font-medium tracking-wide">
        <Endpoint provider={item.fromProvider} label={from} />
        <ArrowRight className="size-3 shrink-0 opacity-60" aria-hidden />
        <Endpoint provider={item.toProvider} label={to} />
        <span className="shrink-0">· {carried}</span>
      </span>
      <span className="h-px flex-1 bg-border/60" />
    </div>
  );
}
