import { useState } from "react";

import { Sparkles } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { useFeatureFlags } from "@/stores/feature-flags";
import { toast } from "@/lib/toast";

/**
 * Step 13 — "BETA FEATURES" Settings section. Single Switch that
 * flips both `enable_agent_chat` and `enable_lazy_workspace_creation`
 * via the `set_agent_chat_beta` Tauri command. Visually distinct from
 * the rest of Settings (warning-tinted card, Sparkles icon, BETA
 * badge) so users can see at a glance that this is preview surface.
 *
 * After the flip, the page reloads — several Settings sections (and
 * pane-tree placeholder branches) need to mount/unmount based on the
 * flag, and a hard reload is the simplest way to engage every gate
 * cleanly without a per-component subscription pass.
 */
export function BetaFeaturesSection() {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const setAgentChatEnabled = useFeatureFlags((s) => s.setAgentChatEnabled);
  const [showDetails, setShowDetails] = useState(false);
  const [pending, setPending] = useState(false);

  const handleToggle = async (next: boolean) => {
    if (pending) return;
    setPending(true);
    try {
      await setAgentChatEnabled(next);
      toast.success(
        next
          ? "Agent Chat enabled — refreshing to apply…"
          : "Agent Chat disabled — refreshing…",
      );
      // Reload after a short delay so the toast is visible. The
      // backend has already persisted the flip, so this is purely a
      // UI-cycle tactic to remount Settings nav rows + pane-tree
      // gates without a per-component subscription chain.
      window.setTimeout(() => window.location.reload(), 300);
    } catch (err) {
      console.error("[beta-features] toggle failed:", err);
      toast.error(`Failed to update Agent Chat: ${String(err)}`);
      setPending(false);
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center gap-2">
        <Sparkles className="size-4 text-warning" />
        <h2 className="text-sm font-semibold uppercase tracking-[0.12em] text-warning">
          Beta Features
        </h2>
      </div>

      <div className="rounded-lg border border-warning/30 bg-warning/5 p-4 space-y-3">
        <div className="flex items-start justify-between gap-4">
          <div className="flex-1 space-y-1.5">
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-medium">Agent Chat</h3>
              <Badge
                variant="outline"
                className="text-[10px] bg-warning/15 text-warning border-warning/30 uppercase tracking-wider"
              >
                Beta
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground leading-relaxed">
              Try the new chat-based agent interface with multi-provider model
              selection, MCP server management, and end-to-end encrypted skills
              sync. Off by default — your existing CLI workflow is unchanged
              when this is off, and your data is preserved if you toggle it
              back off later.
            </p>
          </div>
          <Switch
            checked={enabled}
            onCheckedChange={handleToggle}
            disabled={pending}
            aria-label="Toggle Agent Chat Beta"
          />
        </div>

        <button
          type="button"
          onClick={() => setShowDetails((v) => !v)}
          className="text-xs text-muted-foreground hover:text-foreground transition-colors"
          aria-expanded={showDetails}
        >
          {showDetails ? "Hide details" : "What's included →"}
        </button>

        {showDetails && (
          <ul className="text-xs text-muted-foreground space-y-1 pt-2 border-t border-warning/20">
            <li>• Chat-based agent interface (Claude, Codex, OpenCode)</li>
            <li>• MCP server runtime and management</li>
            <li>• Cross-provider skills system with E2E encrypted sync</li>
            <li>• Mode pills (Plan, Debug, Ask) and Shift+Tab cycling</li>
            <li>• File / folder / GitHub issue / PR / image attachments</li>
            <li>• Multi-provider model picker with favorites</li>
            <li>• Permissions UI for tool-call rules</li>
            <li>• Home-screen chat landing on empty workspaces</li>
          </ul>
        )}
      </div>
    </div>
  );
}
