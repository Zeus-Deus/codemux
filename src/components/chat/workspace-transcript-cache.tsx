import { useMemo, type ReactNode } from "react";
import { useAppStore } from "@/stores/app-store";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext, transcriptCacheBinding } from "./transcript-cache-binding";

/** Generic, light shell: do not import ChatTranscript/MessageList here. They
 * must remain behind AgentChatPane's existing lazy import for startup. */
export function WorkspaceTranscriptCache({ workspace, enabled, children }: {
  workspace: WorkspaceSnapshot;
  enabled: boolean;
  children: ReactNode;
}) {
  const key = enabled ? transcriptCacheBinding(workspace)?.key : null;
  // Subscribe to binding identities, not fresh snapshot objects or messages.
  // Hidden deletion/conversion/rebinding evicts without visiting that workspace.
  const signature = useAppStore((state) => key ? JSON.stringify(
    (state.appState?.workspaces ?? []).flatMap((candidate) => {
      const binding = transcriptCacheBinding(candidate);
      return binding ? [binding.key] : [];
    }),
  ) : "[]");
  const validKeys: string[] = useMemo(() => JSON.parse(signature), [signature]);
  const binding = useMemo(() => key ? transcriptCacheBinding(workspace) : null, [key]);
  // Keep child ancestry stable even when caching is disabled, so adding a
  // second tab does not remount a live Composer/session as a side effect.
  return <TranscriptCacheProvider activeKey={binding?.key ?? null} validKeys={validKeys}>
    <TranscriptBindingContext.Provider value={binding}>{children}</TranscriptBindingContext.Provider>
  </TranscriptCacheProvider>;
}
