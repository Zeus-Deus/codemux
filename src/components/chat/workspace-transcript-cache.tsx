import { useMemo, type ReactNode } from "react";
import { useAppStore } from "@/stores/app-store";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { TranscriptCacheProvider } from "./transcript-cache";
import { TranscriptBindingContext, transcriptCacheBinding, transcriptCacheBindings } from "./transcript-cache-binding";

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
  // Selection can be an editor/browser/split while other chat tabs stay valid.
  const signature = useAppStore((state) => enabled ? JSON.stringify(
    (state.appState?.workspaces ?? []).flatMap((candidate) =>
      transcriptCacheBindings(candidate).map((binding) => binding.key)),
  ) : "[]");
  const validKeys: string[] = useMemo(() => JSON.parse(signature), [signature]);
  const binding = useMemo(() => key ? transcriptCacheBinding(workspace) : null, [key]);
  // Keep child ancestry stable even when caching is disabled, so adding a
  // second tab does not remount a live Composer/session as a side effect.
  return <TranscriptCacheProvider activeKey={binding?.key ?? null} validKeys={validKeys}>
    <TranscriptBindingContext.Provider value={binding}>{children}</TranscriptBindingContext.Provider>
  </TranscriptCacheProvider>;
}
