/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import { agentSecretRequestAnswer } from "@/tauri/commands";
import type { SecretRequest } from "@/tauri/types";
import { SecretRequestPanel, unencryptedConnection } from "./SecretRequestCard";

vi.mock("@/tauri/commands", () => ({ agentSecretRequestAnswer: vi.fn() }));

const request = (id = "r1"): SecretRequest => ({
  id,
  thread_id: "chat",
  label: "Fly.io API token",
  reason: "To deploy the preview app.",
  placeholder: "fo1_…",
  expires_at_ms: Date.now() + 60_000,
});

afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(agentSecretRequestAnswer).mockResolvedValue(undefined);
});

describe("secret request card", () => {
  it("renders nothing without an open request", () => {
    const { container } = render(<SecretRequestPanel requests={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("saves the value exactly as typed, once", async () => {
    render(<SecretRequestPanel requests={[request()]} />);
    const field = screen.getByLabelText("Fly.io API token");
    const save = screen.getByRole("button", { name: "Save securely" });
    expect(save).toBeDisabled();
    fireEvent.change(field, { target: { value: " pass word " } });
    fireEvent.click(save);
    fireEvent.submit(field.closest("form")!);
    await waitFor(() => expect(field).toHaveValue(""));
    expect(agentSecretRequestAnswer).toHaveBeenCalledTimes(1);
    expect(agentSecretRequestAnswer).toHaveBeenCalledWith("r1", {
      type: "save",
      secret: " pass word ",
    });
  });

  it("declines and shows the backend's error", async () => {
    vi.mocked(agentSecretRequestAnswer).mockRejectedValueOnce(
      "This request has ended. Ask the agent again if it still needs it.",
    );
    render(<SecretRequestPanel requests={[request(), request("r2")]} />);
    expect(screen.getByText("1 more after this")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Decline" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This request has ended");
    expect(agentSecretRequestAnswer).toHaveBeenCalledWith("r1", { type: "decline" });
  });

  it("never echoes a non-string failure", async () => {
    vi.mocked(agentSecretRequestAnswer).mockRejectedValueOnce(new Error("sk-secret leaked"));
    render(<SecretRequestPanel requests={[request()]} />);
    fireEvent.change(screen.getByLabelText("Fly.io API token"), { target: { value: "sk-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "Save securely" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save it. Try again.");
  });

  it("blocks entry only on remote pages served as plain HTTP across the network", () => {
    const at = (protocol: string, hostname: string) => ({ protocol, hostname });
    expect(unencryptedConnection(false, at("http:", "192.168.1.5"))).toBe(false);
    for (const [protocol, host] of [
      ["https:", "app.codemux.org"],
      ["https:", "192.168.1.5"],
      ["http:", "localhost"],
      ["http:", "127.0.0.1"],
      ["http:", "[::1]"],
      ["http:", "LocalHost."],
      ["https:", "desk.tail1234.ts.net"],
    ]) {
      expect(unencryptedConnection(true, at(protocol, host)), host).toBe(false);
    }
    // Shared address space (any CGNAT or VPN network) proves no encryption.
    for (const host of ["192.168.1.5", "10.0.0.2", "100.101.102.104", "desk.tail1234.ts.net", "[fd7a:115c:a1e0::1]"]) {
      expect(unencryptedConnection(true, at("http:", host)), host).toBe(true);
    }
  });
});
