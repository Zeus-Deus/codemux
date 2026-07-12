/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useRemoteConnectionStore } from "@/remote/remote-connection-store";
import {
  RemoteConnectionBanner,
  RemoteConnectionChip,
} from "./remote-connection-indicator";

function setRemote(on: boolean) {
  (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = on
    ? true
    : undefined;
}

function renderChip() {
  return render(
    <TooltipProvider delayDuration={0}>
      <RemoteConnectionChip />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  useRemoteConnectionStore.setState({
    status: null,
    host: "",
    offlineMessage: null,
  });
  setRemote(true);
});

afterEach(() => {
  cleanup();
  setRemote(false);
});

describe("RemoteConnectionChip", () => {
  it("renders a quiet 'Remote' chip when connected on the web client", () => {
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    renderChip();
    const chip = screen.getByTestId("remote-connection-chip");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveTextContent("Remote");
    // Steady state is quiet: it must not scream "polite" live-region updates.
    expect(chip).toHaveAttribute("aria-live", "off");
  });

  it("carries the full host in its accessible label + tooltip (not a floating pill)", async () => {
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    renderChip();
    const chip = screen.getByTestId("remote-connection-chip");
    expect(chip).toHaveAttribute(
      "aria-label",
      "Remote — connected to 127.0.0.1:4379",
    );
    await userEvent.hover(chip);
    await waitFor(() =>
      expect(
        screen.getAllByText("Remote — connected to 127.0.0.1:4379").length,
      ).toBeGreaterThan(0),
    );
  });

  it("renders nothing on desktop (non-remote), even if the store somehow has a status", () => {
    setRemote(false);
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    renderChip();
    expect(screen.queryByTestId("remote-connection-chip")).toBeNull();
  });

  it("renders nothing for the degraded states (those belong to the banner)", () => {
    useRemoteConnectionStore.getState().setReconnecting("127.0.0.1:4379");
    renderChip();
    expect(screen.queryByTestId("remote-connection-chip")).toBeNull();

    useRemoteConnectionStore.getState().setOffline();
    expect(screen.queryByTestId("remote-connection-chip")).toBeNull();
  });
});

describe("RemoteConnectionBanner", () => {
  it("shows a loud amber reconnecting banner while backing off", () => {
    useRemoteConnectionStore.getState().setReconnecting("127.0.0.1:4379");
    render(<RemoteConnectionBanner />);
    const banner = screen.getByTestId("remote-connection-banner");
    expect(banner).toHaveTextContent("Reconnecting to 127.0.0.1:4379…");
    expect(banner).toHaveAttribute("data-state", "reconnecting");
    expect(banner).toHaveAttribute("aria-live", "polite");
    // Loud = centered fixed overlay, never the old bottom-left anchor.
    expect(banner.className).toContain("fixed");
    expect(banner.className).toContain("-translate-x-1/2");
    expect(banner.className).toContain("border-status-working/45");
  });

  it("shows a loud red banner with the revoked message when offline", () => {
    useRemoteConnectionStore.getState().setOffline("Remote access revoked");
    render(<RemoteConnectionBanner />);
    const banner = screen.getByTestId("remote-connection-banner");
    expect(banner).toHaveTextContent("Remote access revoked");
    expect(banner).toHaveAttribute("data-state", "offline");
    expect(banner.className).toContain("border-status-attention/40");
  });

  it("renders nothing while connected (the quiet chip owns that state)", () => {
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    render(<RemoteConnectionBanner />);
    expect(screen.queryByTestId("remote-connection-banner")).toBeNull();
  });

  it("renders nothing on desktop (non-remote)", () => {
    setRemote(false);
    useRemoteConnectionStore.getState().setReconnecting("127.0.0.1:4379");
    render(<RemoteConnectionBanner />);
    expect(screen.queryByTestId("remote-connection-banner")).toBeNull();
  });
});
