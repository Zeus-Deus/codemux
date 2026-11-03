import { describe, expect, it } from "vitest";
import {
  expiryTimerDelay,
  localSetupCommands,
  MAX_TIMER_DELAY_MS,
  shellArg,
  tailscaleServeCommand,
} from "./outside-assistants-setup";

describe("shellArg", () => {
  it("passes a plain executable through unquoted", () => {
    expect(shellArg("/usr/bin/codemux")).toBe("/usr/bin/codemux");
    expect(shellArg("codemux")).toBe("codemux");
  });

  it("single-quotes POSIX paths with spaces or quotes", () => {
    expect(shellArg("/opt/My Apps/codemux")).toBe("'/opt/My Apps/codemux'");
    expect(shellArg("it's")).toBe("'it'\\''s'");
    expect(shellArg("/tmp/$HOME/codemux")).toBe("'/tmp/$HOME/codemux'");
    expect(shellArg("")).toBe("''");
  });

  it("double-quotes Windows paths", () => {
    expect(shellArg("C:\\Program Files\\Codemux\\codemux.exe")).toBe('"C:\\Program Files\\Codemux\\codemux.exe"');
    expect(shellArg("\\\\server\\share\\codemux.exe")).toBe('"\\\\server\\share\\codemux.exe"');
  });
});

describe("localSetupCommands", () => {
  it("builds each local command with --client and the quoted executable", () => {
    expect(localSetupCommands("/opt/My Apps/codemux").map(({ id, label, command, copyLabel }) => [id, label, command, copyLabel])).toEqual([
      ["claude-code", "Claude Code", "claude mcp add --scope user codemux -- '/opt/My Apps/codemux' mcp --client claude-code", "Copy Claude Code command"],
      ["codex", "Codex", "codex mcp add codemux -- '/opt/My Apps/codemux' mcp --client codex", "Copy Codex command"],
      ["hermes", "Hermes", "hermes mcp add codemux --command '/opt/My Apps/codemux' --args mcp --client hermes --access supervised", "Copy Hermes command"],
      ["other", "Other", "'/opt/My Apps/codemux' mcp --client my-agent", "Copy command"],
    ]);
    expect(localSetupCommands("codemux").map((row) => row.command)).toEqual([
      "claude mcp add --scope user codemux -- codemux mcp --client claude-code",
      "codex mcp add codemux -- codemux mcp --client codex",
      "hermes mcp add codemux --command codemux --args mcp --client hermes --access supervised",
      "codemux mcp --client my-agent",
    ]);
  });

  it("starts Hermes supervised and leaves the others at the local default", () => {
    const withAccess = localSetupCommands("codemux").filter((row) => row.command.includes("--access"));
    expect(withAccess.map((row) => row.id)).toEqual(["hermes"]);
    expect(withAccess[0].command).toMatch(/--access supervised$/);
  });
});

it("uses the configured port in the tailscale command", () => {
  expect(tailscaleServeCommand(4378)).toBe("tailscale serve --bg 4378");
  expect(tailscaleServeCommand(5000)).toBe("tailscale serve --bg 5000");
});

describe("expiryTimerDelay", () => {
  const now = Date.parse("2026-11-03T12:00:00Z");
  it("clamps a far-future expiry to the browser timer range", () => {
    expect(expiryTimerDelay([new Date(now + 30 * 86_400_000).toISOString()], now)).toBe(MAX_TIMER_DELAY_MS);
  });
  it("waits for the soonest expiry and fires at once for one already past", () => {
    expect(expiryTimerDelay([new Date(now + 300_000).toISOString(), new Date(now + 60_000).toISOString()], now)).toBe(60_000);
    expect(expiryTimerDelay([new Date(now - 1).toISOString(), new Date(now + 60_000).toISOString()], now)).toBe(0);
  });
  it("gives null when nothing expires", () => {
    expect(expiryTimerDelay([], now)).toBeNull();
    expect(expiryTimerDelay(["not a date"], now)).toBeNull();
  });
});
