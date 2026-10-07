import type { TerminalSessionSnapshot, TerminalStatusPayload } from "@/tauri/types";

/** Which glyph the status card shows beside its heading. */
export type TerminalStatusIndicator = "spinner" | "warning" | "exited";

/** Recovery actions the card offers. `restart` respawns a finished shell,
 *  `retry` respawns one that never came up (and offers to copy the error). */
export type TerminalStatusActions = "none" | "restart" | "retry";

export interface TerminalStatusView {
  heading: string;
  indicator: TerminalStatusIndicator;
  exitCode: { label: string; tone: "success" | "danger" } | null;
  actions: TerminalStatusActions;
}

/** What the status card says for a non-ready lifecycle state. */
export function describeTerminalStatus(
  status: Pick<TerminalStatusPayload, "state" | "exit_code">,
): TerminalStatusView {
  const exitCode =
    status.exit_code === null
      ? null
      : {
          label: `Exit code ${status.exit_code}`,
          tone: status.exit_code === 0 ? ("success" as const) : ("danger" as const),
        };
  switch (status.state) {
    case "failed":
      return { heading: "Terminal unavailable", indicator: "warning", exitCode, actions: "retry" };
    case "exited":
      return { heading: "Process exited", indicator: "exited", exitCode, actions: "restart" };
    case "migrating":
      return { heading: "Migrating workspace", indicator: "spinner", exitCode: null, actions: "none" };
    default:
      return { heading: "Terminal starting", indicator: "spinner", exitCode: null, actions: "none" };
  }
}

/**
 * Reconcile the status read when a pane mounts with the session's record in
 * app state.
 *
 * The backend answers "exited" for any session it holds no runtime for, which
 * is also true of a session whose shell has not been spawned yet (a workspace
 * still hydrating on activation). The app-state record only says exited once a
 * real exit was reported, so anything else is still on its way up. A real exit
 * also keeps its code there after the runtime is gone.
 */
export function reconcileMountedStatus(
  status: TerminalStatusPayload,
  session: Pick<TerminalSessionSnapshot, "state" | "last_message" | "exit_code"> | undefined,
): TerminalStatusPayload {
  if (status.state !== "exited" || !session) return status;
  if (session.state === "exited" || session.state === "failed") {
    return {
      ...status,
      state: session.state,
      message: session.last_message ?? status.message,
      exit_code: status.exit_code ?? session.exit_code,
    };
  }
  return { ...status, state: "starting", message: "Starting shell...", exit_code: null };
}
