/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const mockProbe = vi.fn();
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  agentChatProviderHealth: (...args: unknown[]) => mockProbe(...args),
}));

import { ChatHomeLanding } from "./ChatHomeLanding";
import { emptyHealthSlot, useProviderHealth } from "@/stores/provider-health-store";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  ProviderHealthReport,
} from "@/tauri/types";
afterEach(cleanup);
beforeEach(() => {
  mockProbe.mockReset();
  useProviderHealth.setState({
    slots: {
      claude: emptyHealthSlot(),
      codex: emptyHealthSlot(),
      cursor: emptyHealthSlot(),
      grok: emptyHealthSlot(),
      hermes: emptyHealthSlot(),
      opencode: emptyHealthSlot(),
    },
  });
  useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: false, localSessionImportOfferDismissed: false });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});
describe("ChatHomeLanding", () => {
  it("offers opt-in import only for first-run local enabled profiles", () => {
    const view = render(<ChatHomeLanding composer={<div />} />);
    fireEvent.click(screen.getByRole("button", { name: "Import recent chats" }));
    expect(useUIStore.getState().showLocalSessionImport).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    useUIStore.setState({ localSessionImportOfferDismissed: false });
    useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("hides import for remote clients and disabled chat", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    const view = render(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
    delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
    useFeatureFlags.setState({ enableAgentChat: false });
    view.rerender(<ChatHomeLanding composer={<div />} />);
    expect(screen.queryByRole("button", { name: "Import recent chats" })).toBeNull();
  });
  it("renders the marquee headline", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div data-testid="composer-slot" />} />,
    );
    const heading = container.querySelector("h1");
    expect(heading?.textContent).toBe("What should we do today?");
  });

  it("renders the composer passed via props", () => {
    const { container } = render(
      <ChatHomeLanding
        composer={<div data-testid="composer-slot-a">slotA</div>}
      />,
    );
    expect(
      container.querySelector('[data-testid="composer-slot-a"]'),
    ).not.toBeNull();
  });

  it("uses only neutral foreground/muted color tokens — no accents", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const html = container.innerHTML;
    expect(html).not.toMatch(/\btext-primary\b/);
    expect(html).not.toMatch(/\bbg-primary\b/);
    expect(html).not.toMatch(/\btext-success\b/);
    expect(html).not.toMatch(/\btext-warning\b/);
    expect(html).not.toMatch(/\btext-danger\b/);
  });

  it("centers content vertically and horizontally", () => {
    const { container } = render(
      <ChatHomeLanding composer={<div />} />,
    );
    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("items-center");
    expect(root.className).toContain("justify-center");
  });

  describe("first-run agent strip", () => {
    const HEALTH: Partial<Record<AgentChatProviderKind, Partial<ProviderHealthReport>>> = {
      claude: { status: "ready", installed: true },
      codex: {
        status: "error",
        installed: true,
        message: "Codex CLI is not authenticated. Run `codex login` and retry.",
      },
      cursor: { status: "error", installed: false, message: "Not on PATH." },
      grok: { status: "error", installed: false, message: "Not on PATH." },
      opencode: { status: "ready", installed: true },
    };

    beforeEach(() => {
      mockProbe.mockImplementation(async (provider: AgentChatProviderKind) => ({
        provider,
        message: null,
        version: null,
        ...HEALTH[provider],
      }));
    });

    async function renderWithAgents(onSelect = vi.fn()) {
      const view = render(
        <ChatHomeLanding
          composer={<div />}
          agents={{ selected: "claude", onSelect }}
        />,
      );
      await act(async () => {
        await new Promise((r) => setTimeout(r, 0));
      });
      return { view, onSelect };
    }

    it("shows each agent's readiness once every probe has answered", async () => {
      const { onSelect } = await renderWithAgents();
      expect(mockProbe).toHaveBeenCalledTimes(5);
      expect(mockProbe).not.toHaveBeenCalledWith("hermes");
      expect(screen.getByTestId("agent-chip-claude")).toHaveAttribute(
        "data-readiness",
        "ready",
      );
      expect(screen.getByTestId("agent-chip-claude")).toHaveAttribute(
        "aria-pressed",
        "true",
      );
      const codex = screen.getByTestId("agent-chip-codex");
      expect(codex).toHaveAttribute("data-readiness", "not_ready");
      expect(codex).toHaveAttribute("title", expect.stringContaining("codex login"));
      expect(screen.getByTestId("agent-chip-cursor")).toBeDisabled();

      fireEvent.click(screen.getByTestId("agent-chip-opencode"));
      expect(onSelect).toHaveBeenCalledWith("opencode");
    });

    it("reserves its row but stays empty while a probe is still running", async () => {
      mockProbe.mockImplementation(() => new Promise(() => {}));
      await renderWithAgents();
      expect(screen.getByTestId("agent-availability-strip")).toBeEmptyDOMElement();
    });

    it("only appears on a first run, and only where the composer can switch", async () => {
      useAppStore.setState({ appState: { workspaces: [{}] } as unknown as AppStateSnapshot });
      const { view } = await renderWithAgents();
      expect(screen.queryByTestId("agent-availability-strip")).toBeNull();
      expect(mockProbe).not.toHaveBeenCalled();
      view.unmount();

      useAppStore.setState({ appState: { workspaces: [] } as unknown as AppStateSnapshot });
      render(<ChatHomeLanding composer={<div />} />);
      expect(screen.queryByTestId("agent-availability-strip")).toBeNull();
    });
  });
});
