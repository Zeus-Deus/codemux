import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { localSetupCommands, tailscaleServeCommand } from "./outside-assistants-setup";

/** The guide Settings → Outside assistants links to. */
const guide = readFileSync(resolve(process.cwd(), "docs/outside-agents.md"), "utf8");

describe("outside-agent guide", () => {
  it("shows every local setup command in the outside-agent guide", () => {
    for (const row of localSetupCommands("codemux")) {
      expect(guide, `${row.label}: ${row.command}`).toContain(row.command);
    }
  });

  it("shows the tailscale command for the default port", () => {
    expect(guide).toContain(tailscaleServeCommand(4378));
  });
});
