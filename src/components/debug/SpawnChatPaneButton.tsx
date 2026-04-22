import { invoke } from "@tauri-apps/api/core";

import { Button } from "@/components/ui/button";
import { toast } from "@/lib/toast";
import { useFeatureFlags } from "@/stores/feature-flags";

/**
 * Dev-only affordance: invokes `dev_agent_chat_spawn_test_pane` to
 * drop a stub agent-chat pane into the active workspace.
 *
 * Renders as null unless BOTH gates pass:
 *
 * 1. `import.meta.env.DEV` — matches the Rust side's
 *    `cfg!(debug_assertions)` gate on the command, so release
 *    builds never see this control.
 * 2. `enableAgentChat` from the feature-flags store, so the button
 *    stays hidden until the flag is flipped on.
 *
 * The `loaded` check avoids a brief flash: the button hides while
 * the initial `get_feature_flags` call is in flight, then either
 * stays hidden (flag off) or appears exactly once (flag on).
 *
 * Temporary — remove once the real chat UI and settings toggle ship.
 */
export function SpawnChatPaneButton() {
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const loaded = useFeatureFlags((s) => s.loaded);

  if (!import.meta.env.DEV) return null;
  if (!loaded || !enableAgentChat) return null;

  const handleClick = async () => {
    try {
      const paneId = await invoke<string>("dev_agent_chat_spawn_test_pane");
      toast.success(`Spawned chat pane: ${paneId}`);
    } catch (err) {
      toast.error(`Failed to spawn chat pane: ${err}`);
    }
  };

  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={handleClick}
      className="w-full justify-start text-xs text-muted-foreground"
      title="Dev only — spawn an agent_chat stub pane in the active workspace"
    >
      + Spawn chat pane (dev)
    </Button>
  );
}
