/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ChatHomeLanding } from "./ChatHomeLanding";

import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";
afterEach(cleanup);
beforeEach(() => {
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
});
