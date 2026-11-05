/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import type { ProviderHealthReport } from "@/tauri/types";
import { ConnectAgentCard } from "./ConnectAgentCard";
import type { FirstRunAgents } from "./first-run-agents";

afterEach(cleanup);

const ready = (provider: "claude" | "codex", auth_label?: string): ProviderHealthReport => ({
  provider,
  status: "ready",
  installed: true,
  message: null,
  version: "1.0.0",
  ...(auth_label ? { auth_label } : {}),
});

function agents(reports: FirstRunAgents["reports"]): FirstRunAgents {
  return {
    providers: ["claude", "codex"],
    reports,
    known: true,
    ready: [],
    unverified: [],
    needsAgent: true,
  };
}

describe("ConnectAgentCard", () => {
  it("says how a ready agent signs in", () => {
    render(<ConnectAgentCard agents={agents({
      claude: ready("claude", "Custom endpoint · gateway.example.com"),
      codex: ready("codex"),
    })} />);
    expect(screen.getByTestId("connect-agent-claude")).toHaveTextContent(
      "Ready · Custom endpoint · gateway.example.com",
    );
    // No label from an older backend: just "Ready".
    expect(screen.getByTestId("connect-agent-codex")).toHaveTextContent(/Ready$/);
  });
});
