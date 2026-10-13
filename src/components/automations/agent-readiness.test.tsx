/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import type { ProviderHealthReport } from "@/tauri/types";
import {
  AgentReadinessRow,
  agentBlocked,
  preferredAutomationAgent,
  type AutomationAgentHealth,
} from "./agent-readiness";

afterEach(cleanup);

function settled(
  provider: "claude" | "codex",
  status: ProviderHealthReport["status"],
  installed = true,
  message: string | null = null,
) {
  return {
    report: { provider, status, installed, message, version: null },
    pending: false,
  };
}

describe("preferredAutomationAgent", () => {
  it("waits until every probe has answered", () => {
    const health: AutomationAgentHealth = {
      claude: settled("claude", "error", false),
      codex: { report: null, pending: true },
    };
    expect(preferredAutomationAgent(health)).toBeNull();
  });

  it("keeps Claude Code when it is ready", () => {
    expect(
      preferredAutomationAgent({
        claude: settled("claude", "ready"),
        codex: settled("codex", "ready"),
      }),
    ).toBe("claude");
  });

  it("moves to Codex when only Codex is ready", () => {
    expect(
      preferredAutomationAgent({
        claude: settled("claude", "error", true, "Run `claude login`."),
        codex: settled("codex", "ready"),
      }),
    ).toBe("codex");
  });

  it("falls back to Claude Code when nothing is ready or the probe failed", () => {
    expect(
      preferredAutomationAgent({
        claude: { report: null, pending: false },
        codex: settled("codex", "error", false),
      }),
    ).toBe("claude");
  });
});

describe("agentBlocked", () => {
  it("blocks only a missing or signed-out agent", () => {
    const health: AutomationAgentHealth = {
      claude: settled("claude", "warning", true, "Could not verify."),
      codex: settled("codex", "error", true, "Run `codex login`."),
    };
    expect(agentBlocked(health, "claude")).toBe(false);
    expect(agentBlocked(health, "codex")).toBe(true);
    expect(agentBlocked({}, "claude")).toBe(false);
  });
});

describe("AgentReadinessRow", () => {
  it("shows the sign-in message for a signed-out local agent", () => {
    render(
      <AgentReadinessRow
        agent="claude"
        local
        health={{
          claude: settled(
            "claude",
            "error",
            true,
            "Claude CLI is not authenticated. Run `claude login` and retry.",
          ),
        }}
      />,
    );
    const row = screen.getByTestId("automation-agent-readiness");
    expect(row).toHaveAttribute("data-readiness", "not_ready");
    expect(row).toHaveTextContent("Run `claude login`");
  });

  it("confirms a ready agent and shows progress while checking", () => {
    const { rerender } = render(
      <AgentReadinessRow
        agent="codex"
        local
        health={{ codex: { report: null, pending: true } }}
      />,
    );
    expect(screen.getByTestId("automation-agent-readiness")).toHaveTextContent(
      "Checking Codex on this machine",
    );
    rerender(
      <AgentReadinessRow
        agent="codex"
        local
        health={{ codex: settled("codex", "ready") }}
      />,
    );
    expect(screen.getByTestId("automation-agent-readiness")).toHaveAttribute(
      "data-readiness",
      "ready",
    );
  });

  it("does not guess for a remote host", () => {
    render(<AgentReadinessRow agent="claude" local={false} health={{}} />);
    const row = screen.getByTestId("automation-agent-readiness");
    expect(row).toHaveAttribute("data-readiness", "unknown");
    expect(row).toHaveTextContent("checked when the automation runs");
  });
});
