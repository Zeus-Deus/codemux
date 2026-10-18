/** Two independent workspace launches, using the real host-tool event shapes. */
import type { AgentChatProviderKind } from "@/tauri/types";

export const workspaceLaunchScenario = (search: string): boolean =>
  new URLSearchParams(search).get("mockScenario") === "workspace-launch";

export interface MockWorkspaceLaunch {
  title: string;
  branch: string;
  message: string;
  provider: AgentChatProviderKind;
  threadId: string;
}

export function streamWorkspaceLaunch(
  parent: string,
  host: {
    emit: (threadId: string, event: unknown) => void;
    openWorkspace: (launch: MockWorkspaceLaunch) => void;
  },
): void {
  const turnId = "mock-workspace-launch";
  const emit = (event: object) => host.emit(parent, { thread_id: parent, turn_id: turnId, ...event });
  const launches: MockWorkspaceLaunch[] = [
    {
      title: "Fix flaky sidebar test",
      branch: "fix-flaky-sidebar-test",
      message: "Fix the flaky sidebar test in src/components/layout/sidebar-workspace-row.test.tsx. Investigate the timing race and preserve the existing behavior. Follow AGENTS.md. Verify with npm run check and npm run test -- src/components/layout/sidebar-workspace-row.test.tsx. Commit the fix; do not open a PR.",
      provider: "claude",
      threadId: "mock-workspace-sidebar",
    },
    {
      title: "Fix terminal resize bug",
      branch: "fix-terminal-resize-bug",
      message: "Fix the terminal resize bug: after switching workspaces the terminal sometimes keeps its old dimensions. Inspect src/components/terminal and the resize IPC path. Follow AGENTS.md and add a focused regression test. Run npm run check and affected tests, plus cargo check if Rust changes. Commit the fix; do not open a PR.",
      provider: "codex",
      threadId: "mock-workspace-resize",
    },
  ];
  emit({ type: "user_message", text: "Start two separate workspaces: one to fix the flaky sidebar test, one to fix the terminal resize bug", client_nonce: "mock-workspace-ask" });
  emit({ type: "session_state_changed", status: { status: "running", active_turn: turnId } });
  launches.forEach((launch, index) => {
    const toolUseId = `workspace-launch-${index}`;
    window.setTimeout(() => {
      emit({ type: "item_completed", item: { kind: "tool_use", tool_name: "mcp__codemux__workspace_launch", tool_use_id: toolUseId, input: { title: launch.title, branch: launch.branch, message: launch.message, provider: launch.provider } } });
      emit({ type: "item_completed", item: { kind: "tool_result", tool_use_id: toolUseId, is_error: false, content: [{ type: "text", text: JSON.stringify({ status: "starting", title: launch.title, branch: launch.branch, base: "main", provider: launch.provider, note: "The workspace appears in the sidebar shortly." }) }] } });
    }, 350 + index * 450);
    window.setTimeout(() => host.openWorkspace(launch), 1000 + index * 450);
  });
  window.setTimeout(() => {
    emit({ type: "item_completed", item: { kind: "assistant_text", text: "Started “Fix flaky sidebar test” and “Fix terminal resize bug”; follow each agent in its workspace in the sidebar." } });
    emit({ type: "turn_completed", status: { kind: "success" }, usage: null });
    emit({ type: "session_state_changed", status: { status: "ready" } });
  }, 1800);
}
