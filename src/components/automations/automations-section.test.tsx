/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ProviderHealthReport } from "@/tauri/types";
import {
  emptyHealthSlot,
  HEALTH_REPROBE_MS,
  useProviderHealth,
} from "@/stores/provider-health-store";
import { AutomationsSection } from "./automations-section";

const { probe } = vi.hoisted(() => ({ probe: vi.fn() }));

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/tauri/commands")>(),
  agentChatProviderHealth: probe,
  automationsList: vi.fn().mockResolvedValue([]),
  hostsList: vi.fn().mockResolvedValue([]),
  automationsCheckRepoAccess: vi.fn().mockResolvedValue({ ok: true }),
}));

function report(
  provider: "claude" | "codex",
  status: ProviderHealthReport["status"],
  installed = true,
): ProviderHealthReport {
  return { provider, status, installed, message: null, version: null };
}

function deferred() {
  let resolve!: (value: ProviderHealthReport) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<ProviderHealthReport>((r, fail) => {
    resolve = r;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function agentTrigger() {
  return screen.getAllByRole("combobox").find((element) =>
    /^(Claude Code|Codex)$/.test(element.textContent ?? ""),
  )!;
}

describe("automation agent defaults", () => {
  beforeEach(() => {
    probe.mockReset();
    const fetchedAt = Date.now() - HEALTH_REPROBE_MS - 1;
    useProviderHealth.setState((state) => ({
      slots: {
        ...state.slots,
        claude: { ...emptyHealthSlot(), report: report("claude", "ready"), fetchedAt },
        codex: { ...emptyHealthSlot(), report: report("codex", "error", false), fetchedAt },
      },
    }));
  });

  afterEach(cleanup);

  async function openForm() {
    const claude = deferred();
    const codex = deferred();
    probe.mockImplementation((provider: string) =>
      provider === "claude" ? claude.promise : codex.promise,
    );
    const user = userEvent.setup();
    render(<AutomationsSection />);
    await user.click(await screen.findByRole("button", { name: "New automation" }));
    expect(probe).toHaveBeenCalledTimes(2);
    return { claude, codex, user };
  }

  async function settle(claude: ReturnType<typeof deferred>, codex: ReturnType<typeof deferred>) {
    await act(async () => {
      claude.resolve(report("claude", "error"));
      await claude.promise;
    });
    expect(agentTrigger()).toHaveTextContent("Claude Code");
    await act(async () => {
      codex.resolve(report("codex", "ready"));
      await codex.promise;
    });
  }

  it("waits for expired cached reports to refresh before choosing a ready agent", async () => {
    const { claude, codex } = await openForm();
    expect(screen.getByTestId("automation-agent-readiness")).toHaveAttribute("data-readiness", "checking");
    await settle(claude, codex);

    expect(agentTrigger()).toHaveTextContent("Codex");
    expect(screen.getByTestId("automation-agent-readiness")).toHaveAttribute("data-readiness", "ready");
  });

  it("preserves an explicit agent choice when the expired reports finish refreshing", async () => {
    const { claude, codex, user } = await openForm();
    await user.click(agentTrigger());
    await user.click(await screen.findByRole("option", { name: "Codex" }));
    await user.click(agentTrigger());
    await user.click(await screen.findByRole("option", { name: "Claude Code" }));

    await settle(claude, codex);

    expect(agentTrigger()).toHaveTextContent("Claude Code");
    expect(screen.getByTestId("automation-agent-readiness")).toHaveAttribute("data-readiness", "not_ready");
  });

  it("chooses from fresh answers after a refresh fails instead of trusting the old ready report", async () => {
    const { claude, codex } = await openForm();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    await act(async () => {
      claude.reject(new Error("IPC down"));
      codex.resolve(report("codex", "ready"));
      await Promise.allSettled([claude.promise, codex.promise]);
    });

    expect(agentTrigger()).toHaveTextContent("Codex");
    expect(screen.getByTestId("automation-agent-readiness")).toHaveAttribute("data-readiness", "ready");
    warn.mockRestore();
  });
});
