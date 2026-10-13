import { openUrl } from "@tauri-apps/plugin-opener";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useChatGptStore } from "@/stores/chatgpt-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { CHATGPT_USAGE_URL } from "@/components/auth/chatgpt-connection";

/** Plan attribution stays next to the provider controls, not the app account. */
export function ChatGptPlanUsage({ provider }: { provider: AgentChatProviderKind }) {
  const connected = useChatGptStore((s) => s.status?.phase === "connected");
  if (provider !== "codex" || !connected || isRemoteClient()) return null;
  return <div className="flex flex-wrap items-center justify-between gap-2 px-3 pb-2 text-label text-muted-foreground">
    <span>Using ChatGPT plan</span>
    <button type="button" className="underline-offset-4 hover:text-foreground hover:underline"
      onClick={() => void openUrl(CHATGPT_USAGE_URL)}>Manage usage</button>
  </div>;
}
