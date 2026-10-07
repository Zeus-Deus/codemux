import type { ReactNode } from "react";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useAppStore } from "@/stores/app-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { AgentAvailabilityStrip } from "./AgentAvailabilityStrip";
import { LocalSessionImportEntry } from "./LocalSessionImportEntry";

interface Props {
  composer: ReactNode;
  /** The composer's provider and how to switch it. Given only by the
   *  draft surface; with it, a first run (no workspaces yet) shows which
   *  agents are installed and signed in on this machine. */
  agents?: {
    selected: AgentChatProviderKind;
    onSelect: (provider: AgentChatProviderKind) => void;
  };
}

/**
 * Empty-state landing for the chat surface. Per the chat-ui skill,
 * this is the sole place inside the chat feature that may exceed
 * prose size — one marquee headline above the composer, no grid of
 * example prompts, no marketing copy. The composer is the invitation.
 */
export function ChatHomeLanding({ composer, agents }: Props) {
  const firstRun = useAppStore(
    (s) => s.appState !== null && (s.appState.workspaces?.length ?? 0) === 0,
  );
  return (
    <div className="flex h-full w-full flex-col items-center justify-center gap-8 pb-12">
      <h1 className="px-4 text-3xl font-medium tracking-tight text-foreground text-center">
        What should we do today?
      </h1>
      {/* The composer carries the shared column rails itself (see
          chat-column.ts), so the landing card lines up with the
          mid-conversation composer at every pane width. */}
      <div className="w-full">{composer}</div>
      {agents && firstRun && !isRemoteClient() && (
        <AgentAvailabilityStrip
          selected={agents.selected}
          onSelect={agents.onSelect}
        />
      )}
      <LocalSessionImportEntry firstRun />
    </div>
  );
}
