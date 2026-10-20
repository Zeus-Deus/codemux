/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { AutomationView } from "@/tauri/commands";
import { WebhookPanel, webhookEndpoints } from "./webhook-panel";

const commands = vi.hoisted(() => ({
  automationsWebhookCredentials: vi.fn(),
  automationsRotateWebhookToken: vi.fn(),
  automationsSetWebhookSignature: vi.fn(),
  secretsStore: vi.fn(),
  secretsDelete: vi.fn(),
  webRemoteStatus: vi.fn(),
  webRemoteListEndpoints: vi.fn(),
}));

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  ...commands,
}));

const TOKEN = "a".repeat(64);

function automation(overrides: Partial<AutomationView> = {}): AutomationView {
  return {
    id: 7,
    server_id: null,
    name: "On issue",
    prompt: "Triage {{body.issue.title}}",
    agent: "claude",
    schedule: "",
    timezone: "UTC",
    host_id: null,
    project_path: "/repo",
    project_remote: null,
    enabled: true,
    retention_limit: 10,
    last_run_at: null,
    next_run_at: null,
    last_run_status: null,
    created_at: "",
    updated_at: "",
    dirty: false,
    trigger_type: "webhook",
    webhook: { id: "hook7", signature: null, secret_ref: null },
    ...overrides,
  };
}

describe("WebhookPanel", () => {
  beforeEach(() => {
    commands.automationsWebhookCredentials.mockResolvedValue({ webhook_id: "hook7", token: TOKEN });
    commands.webRemoteStatus.mockResolvedValue({ enabled: true, running: true });
    commands.webRemoteListEndpoints.mockResolvedValue([
      {
        kind: "lan",
        group: "local_network",
        host: "192.168.1.20",
        port: 4377,
        url: "http://192.168.1.20:4377",
        secure: false,
        recommended: true,
        label: "Local network",
      },
    ]);
    commands.secretsDelete.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("masks the URL token until revealed", async () => {
    const user = userEvent.setup();
    render(<WebhookPanel automation={automation()} onChanged={vi.fn()} />);
    const url = await screen.findByTestId("webhook-url");
    await waitFor(() =>
      expect(url).toHaveTextContent("http://192.168.1.20:4377/hooks/automations/hook7/"),
    );
    expect(url).not.toHaveTextContent(TOKEN);

    await user.click(screen.getByRole("button", { name: "Show token" }));
    expect(url).toHaveTextContent(TOKEN);
  });

  it("warns when Remote access is not listening", async () => {
    commands.webRemoteStatus.mockResolvedValue({ enabled: false, running: false });
    render(<WebhookPanel automation={automation()} onChanged={vi.fn()} />);
    expect(await screen.findByTestId("webhook-listener-off")).toHaveTextContent(
      "Remote access is off",
    );
  });

  it("stores the secret and saves the check by reference only", async () => {
    commands.secretsStore.mockResolvedValue({ id: "secret_new", label: "x", created_at: "" });
    const updated = automation({
      webhook: {
        id: "hook7",
        signature: { header: "x-hub-signature-256", encoding: "hex", prefix: "sha256=" },
        secret_ref: "secret_new",
      },
    });
    commands.automationsSetWebhookSignature.mockResolvedValue(updated);
    const onChanged = vi.fn();
    const user = userEvent.setup();
    render(<WebhookPanel automation={automation()} onChanged={onChanged} />);

    await user.click(await screen.findByRole("button", { name: /Add check/ }));
    const input = screen.getByLabelText("Signing secret");
    expect(input).toHaveAttribute("type", "password");
    await user.type(input, "  s3cret ");
    await user.click(screen.getByRole("button", { name: "Save check" }));

    await waitFor(() => expect(onChanged).toHaveBeenCalledWith(updated));
    // The value goes to the store byte for byte; the check gets only its id.
    expect(commands.secretsStore).toHaveBeenCalledWith("On issue webhook (#7)", "  s3cret ");
    expect(commands.automationsSetWebhookSignature).toHaveBeenCalledWith(7, {
      header: "x-hub-signature-256",
      encoding: "hex",
      prefix: "sha256=",
      secret_ref: "secret_new",
    });
  });

  it("keeps the secret label within the store's 200-byte limit for a long name", async () => {
    // Names may be up to 200 bytes, and the secret label has the same limit.
    // Emoji are surrogate pairs, so a cut must not split one.
    const name = "😀".repeat(49) + "x";
    commands.secretsStore.mockResolvedValue({ id: "secret_new", label: "x", created_at: "" });
    commands.automationsSetWebhookSignature.mockResolvedValue(automation({ name }));
    const user = userEvent.setup();
    render(<WebhookPanel automation={automation({ id: 4242, name })} onChanged={vi.fn()} />);

    await user.click(await screen.findByRole("button", { name: /Add check/ }));
    await user.type(screen.getByLabelText("Signing secret"), "s3cret");
    await user.click(screen.getByRole("button", { name: "Save check" }));

    await waitFor(() => expect(commands.secretsStore).toHaveBeenCalled());
    const label = commands.secretsStore.mock.calls[0][0] as string;
    expect(new TextEncoder().encode(label).length).toBeLessThanOrEqual(200);
    expect(label).toMatch(/^(?:😀)+ webhook \(#4242\)$/u);
  });

  it("deletes a stored secret when saving its check fails", async () => {
    commands.secretsStore.mockResolvedValue({ id: "secret_orphan", label: "x", created_at: "" });
    commands.automationsSetWebhookSignature.mockRejectedValue("The signature header name is not a valid HTTP header");
    const user = userEvent.setup();
    render(<WebhookPanel automation={automation()} onChanged={vi.fn()} />);

    await user.click(await screen.findByRole("button", { name: /Add check/ }));
    await user.type(screen.getByLabelText("Signing secret"), "s3cret");
    await user.click(screen.getByRole("button", { name: "Save check" }));

    expect(await screen.findByText(/not a valid HTTP header/)).toBeInTheDocument();
    expect(commands.secretsDelete).toHaveBeenCalledWith("secret_orphan");
  });

  it("replacing a check deletes the secret it no longer uses", async () => {
    const current = automation({
      webhook: {
        id: "hook7",
        signature: { header: "x-hub-signature-256", encoding: "hex", prefix: "sha256=" },
        secret_ref: "secret_old",
      },
    });
    commands.secretsStore.mockResolvedValue({ id: "secret_new", label: "x", created_at: "" });
    commands.automationsSetWebhookSignature.mockResolvedValue(current);
    const user = userEvent.setup();
    render(<WebhookPanel automation={current} onChanged={vi.fn()} />);

    await user.click(await screen.findByRole("button", { name: /Change/ }));
    await user.type(screen.getByLabelText("Signing secret"), "rotated");
    await user.click(screen.getByRole("button", { name: "Save check" }));

    await waitFor(() => expect(commands.secretsDelete).toHaveBeenCalledWith("secret_old"));
  });

  it("only offers addresses the listener serves under its bind scope", () => {
    const endpoint = (group: string, url: string) => ({
      kind: "lan",
      group,
      host: new URL(url).hostname.replace(/^\[|\]$/g, ""),
      port: 4377,
      url,
      secure: false,
      recommended: false,
      label: "",
    });
    const all = [
      endpoint("this_device", "http://127.0.0.1:4377"),
      endpoint("local_network", "http://192.168.1.20:4377"),
      endpoint("tailscale", "https://box.tail.ts.net"),
      endpoint("other", "iroh://node"),
      endpoint("local_network", "http://[fe80::1]:4377"),
      endpoint("tailscale", "http://[fd7a::5]:4377"),
    ];
    const urls = (scope?: "all" | "tailscale" | "loopback") =>
      webhookEndpoints(all, scope).map((e) => e.url);
    expect(urls("all")).toEqual([
      "http://127.0.0.1:4377",
      "http://192.168.1.20:4377",
      "https://box.tail.ts.net",
    ]);
    expect(urls(undefined)).toEqual(urls("all"));
    expect(urls("tailscale")).toEqual([
      "http://127.0.0.1:4377",
      "https://box.tail.ts.net",
      "http://[fd7a::5]:4377",
    ]);
    expect(urls("loopback")).toEqual(["http://127.0.0.1:4377"]);
  });
});
