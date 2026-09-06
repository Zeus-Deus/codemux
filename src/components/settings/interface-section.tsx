import { useState } from "react";

import { Check, Copy, Gauge, MessageSquareText } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

import { Switch } from "@/components/ui/switch";
import { useFeatureFlags } from "@/stores/feature-flags";
import { toast } from "@/lib/toast";
import { copyToClipboard, COPY_FAILED_MESSAGE } from "@/lib/clipboard";
import { collectPerformanceDiagnostics } from "@/lib/perf/performance-diagnostics";

/**
 * Settings → Interface. Home of the Agent Chat GUI master toggle —
 * formerly the "Beta Features" section, promoted to a regular setting
 * now that the GUI is the default interface.
 *
 * One Switch flips both `enable_agent_chat` and
 * `enable_lazy_workspace_creation` via the `set_agent_chat_enabled`
 * Tauri command (the backend writes both fields under one mutex
 * acquisition). Default is ON; turning it off returns Codemux to the
 * classic terminal-first (CLI) interface. Nothing is lost either way —
 * chat sessions, panes, and layouts are preserved across flips.
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
export function InterfaceSection() {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const setAgentChatEnabled = useFeatureFlags((s) => s.setAgentChatEnabled);
  const [showDetails, setShowDetails] = useState(false);
  const [pending, setPending] = useState(false);
  const [copyingDiagnostics, setCopyingDiagnostics] = useState(false);
  const [diagnosticsCopied, setDiagnosticsCopied] = useState(false);

  const copyDiagnostics = async () => {
    if (copyingDiagnostics) return;
    setCopyingDiagnostics(true);
    try {
      const report = await collectPerformanceDiagnostics();
      const copied = await copyToClipboard(JSON.stringify(report, null, 2));
      if (!copied) {
        toast.error(COPY_FAILED_MESSAGE);
        return;
      }
      setDiagnosticsCopied(true);
      window.setTimeout(() => setDiagnosticsCopied(false), 1_400);
      toast.success("Performance diagnostics copied");
    } catch (error) {
      toast.error(`Couldn't collect performance diagnostics: ${String(error)}`);
    } finally {
      setCopyingDiagnostics(false);
    }
  };

  const handleToggle = async (next: boolean) => {
    if (pending) return;
    setPending(true);
    try {
      await setAgentChatEnabled(next);
      toast.success(
        next
          ? "Agent Chat GUI enabled — Codemux will close. Reopen to apply."
          : "Switched to the classic CLI interface — Codemux will close. Reopen to apply.",
        { duration: 4_000 },
      );
      // Short delay so the toast is visible before the window goes
      // away. Backend has already persisted the flip, so even if the
      // user kills the process before this fires the new state is
      // safe on disk and takes effect on next launch.
      window.setTimeout(() => {
        void invoke<void>("quit_app").catch((err) => {
          console.error("[interface-section] quit_app failed:", err);
          toast.error(
            `Couldn't close Codemux automatically — please quit and reopen manually: ${String(err)}`,
          );
          setPending(false);
        });
      }, 600);
    } catch (err) {
      console.error("[interface-section] toggle failed:", err);
      toast.error(`Failed to update the interface setting: ${String(err)}`);
      setPending(false);
    }
  };

  return (
    <div>
      <div className="mb-7">
        <h2 className="text-[21px] font-bold tracking-tight text-foreground">
          Interface
        </h2>
        <p className="text-[14px] text-muted-foreground/80 mt-1.5 leading-relaxed max-w-prose">
          Choose how Codemux presents agent sessions.
        </p>
      </div>

      <div className="rounded-lg border border-border bg-card p-4 space-y-3">
        <div className="flex items-start justify-between gap-4">
          <div className="flex-1 space-y-1.5">
            <div className="flex items-center gap-2">
              <MessageSquareText className="size-4 text-muted-foreground" />
              <h3 className="text-sm font-medium">Agent Chat GUI</h3>
            </div>
            <p className="text-xs text-muted-foreground leading-relaxed">
              The chat-based agent interface with multi-provider model
              selection, MCP server management, and skills sync. On by
              default — turn it off to go back to the classic terminal-first
              (CLI) interface. Your data is preserved either way, and you can
              still launch CLI agents from the GUI — click one in the +
              launcher to open it in a new tab.
            </p>
          </div>
          <Switch
            checked={enabled}
            onCheckedChange={handleToggle}
            disabled={pending}
            aria-label="Toggle Agent Chat GUI"
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
          <ul className="text-xs text-muted-foreground space-y-1 pt-2 border-t border-border">
            <li>
              • Chat-based agent interface (Claude, Codex, Cursor, Grok,
              OpenCode)
            </li>
            <li>• MCP server runtime and management</li>
            <li>• Cross-provider skills system with cross-device sync</li>
            <li>• Mode pills (Plan, Debug, Ask) and Shift+Tab cycling</li>
            {/* Brand-free: the same attachment kinds work on every
                hosting product Codemux supports. */}
            <li>• File / folder / issue / PR / image attachments</li>
            <li>• Multi-provider model picker with favorites</li>
            <li>• Permissions UI for tool-call rules</li>
            <li>• Home-screen chat landing on empty workspaces</li>
          </ul>
        )}
      </div>

      <div className="mt-4 rounded-lg border border-border bg-card p-4">
        <div className="flex items-center justify-between gap-4">
          <div className="min-w-0 space-y-1">
            <div className="flex items-center gap-2">
              <Gauge className="size-4 text-muted-foreground" />
              <h3 className="text-sm font-medium">Performance diagnostics</h3>
            </div>
            <p className="text-xs text-muted-foreground leading-relaxed">
              Copy bounded startup, workspace-switch, renderer, payload-size,
              and native timing summaries. Paths, titles, messages, and IDs are excluded.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void copyDiagnostics()}
            disabled={copyingDiagnostics}
            className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-background px-3 py-2 text-xs font-medium hover:bg-muted disabled:opacity-50"
          >
            {diagnosticsCopied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
            {diagnosticsCopied ? "Copied" : copyingDiagnostics ? "Collecting…" : "Copy performance diagnostics"}
          </button>
        </div>
      </div>
    </div>
  );
}
