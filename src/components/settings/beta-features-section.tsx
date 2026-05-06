import { useState } from "react";

import { Sparkles } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

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
 * After the flip the app quits via the `quit_app` Tauri command and
 * the user reopens it manually. We tried auto-restart (detached
 * spawn, setsid, /dev/null stdio, control-socket teardown) but the
 * dev-server WebView path can't survive the cargo runner exiting,
 * and a "dev: rerun manually / prod: auto-restart" split was a
 * landmine. Plain quit is the honest UX: every user — dev or
 * production — sees the same flow, and the next launch is
 * unambiguously a clean boot under the new flag state.
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
          ? "Agent Chat enabled — Codemux will close. Reopen to apply."
          : "Agent Chat disabled — Codemux will close. Reopen to apply.",
        { duration: 4_000 },
      );
      // Short delay so the toast is visible before the window goes
      // away. Backend has already persisted the flip, so even if the
      // user kills the process before this fires the new state is
      // safe on disk and takes effect on next launch.
      window.setTimeout(() => {
        void invoke<void>("quit_app").catch((err) => {
          console.error("[beta-features] quit_app failed:", err);
          toast.error(
            `Couldn't close Codemux automatically — please quit and reopen manually: ${String(err)}`,
          );
          setPending(false);
        });
      }, 600);
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
