import { describe, expect, it } from "vitest";

import { describeTerminalStatus, reconcileMountedStatus } from "./terminal-status-view";

describe("describeTerminalStatus", () => {
  it("presents an exited shell as finished, with its exit code and a restart", () => {
    expect(describeTerminalStatus({ state: "exited", exit_code: 0 })).toEqual({
      heading: "Process exited",
      indicator: "exited",
      exitCode: { label: "Exit code 0", tone: "success" },
      actions: "restart",
    });
    expect(describeTerminalStatus({ state: "exited", exit_code: 130 }).exitCode).toEqual({
      label: "Exit code 130",
      tone: "danger",
    });
  });

  it("offers a retry for a failed terminal", () => {
    expect(describeTerminalStatus({ state: "failed", exit_code: null })).toMatchObject({
      heading: "Terminal unavailable",
      indicator: "warning",
      actions: "retry",
    });
  });

  it("keeps the spinner and no actions while the shell is coming up", () => {
    expect(describeTerminalStatus({ state: "starting", exit_code: null })).toMatchObject({
      heading: "Terminal starting",
      indicator: "spinner",
      actions: "none",
    });
    expect(describeTerminalStatus({ state: "migrating", exit_code: null }).heading).toBe(
      "Migrating workspace",
    );
  });
});

describe("reconcileMountedStatus", () => {
  const runtimeless = {
    session_id: "s",
    state: "exited" as const,
    message: "Session is no longer running",
    exit_code: null,
  };

  it("keeps a real exit, restoring its code and message from app state", () => {
    expect(
      reconcileMountedStatus(runtimeless, {
        state: "exited",
        last_message: "Shell exited with code 1",
        exit_code: 1,
      }),
    ).toEqual({
      session_id: "s",
      state: "exited",
      message: "Shell exited with code 1",
      exit_code: 1,
    });
  });

  it("reads a session that has not been spawned yet as starting", () => {
    expect(
      reconcileMountedStatus(runtimeless, { state: "starting", last_message: null, exit_code: null })
        .state,
    ).toBe("starting");
  });

  it("leaves live states alone", () => {
    const ready = { session_id: "s", state: "ready" as const, message: null, exit_code: null };
    expect(reconcileMountedStatus(ready, undefined)).toBe(ready);
  });
});
