/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderUpdateNotice } from "./ProviderUpdateNotice";
import { useProviderUpdates, updateTargetKey, UPDATE_INTERVAL, type ProviderUpdateReport } from "@/stores/provider-update-store";
import { useHermes } from "@/stores/hermes-store";
import type { AgentChatProviderKind } from "@/tauri/types";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
const report: ProviderUpdateReport = { provider: "codex", installed_version: "0.157.0", latest_version: "0.158.0", available: true, manager: "Omarchy · mise", can_update: true, message: null };
beforeEach(() => { useProviderUpdates.setState({ slots: {} }); invoke.mockReset(); invoke.mockResolvedValue(report); });
afterEach(() => { cleanup(); vi.useRealTimers(); });
describe("ProviderUpdateNotice", () => {
  it.each<AgentChatProviderKind>(["claude", "codex", "cursor", "grok", "hermes", "opencode"])("offers updates for %s", async (provider) => {
    useHermes.setState({ selections: { demo: { schema_version: 1, host: "local", installation: "/tools/hermes", root: "/profiles", id: "default", home: "/profiles", identity: "1" } } });
    invoke.mockResolvedValue({ ...report, provider });
    render(<ProviderUpdateNotice provider={provider} threadId="demo" />);
    expect(await screen.findByRole("button", { name: "Update" })).toBeVisible();
    expect(screen.getByText(/Managed by Omarchy/)).toBeVisible();
  });
  it.each(["npm", "pnpm", "Bun", "Homebrew", "mise", "Claude updater"])("offers an update through %s outside Omarchy", async (manager) => {
    invoke.mockResolvedValue({ ...report, manager });
    render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    expect(await screen.findByRole("button", { name: "Update" })).toBeVisible();
    expect(screen.getByText(`Updates through ${manager}. Restart Codemux afterward to use the new version.`)).toBeVisible();
    expect(screen.queryByText(/Managed by Omarchy/)).not.toBeInTheDocument();
  });
  it("shows progress, errors, retry and verified success", async () => {
    render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    const button = await screen.findByRole("button", { name: "Update" });
    let reject!: (error: Error) => void;
    invoke.mockReturnValueOnce(new Promise((_, r) => { reject = r; }));
    fireEvent.click(button);
    expect(screen.getByRole("button", { name: "Updating…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Dismiss Codex update" })).toBeDisabled();
    await act(async () => reject(new Error("Network unavailable")));
    expect(await screen.findByRole("alert")).toHaveTextContent("Network unavailable");
    invoke.mockResolvedValueOnce({ ...report, installed_version: "0.158.0", available: false });
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("Codex updated")).toBeVisible();
    expect(screen.getByText(/Restart Codemux when/)).toBeVisible();
  });
  it("dismisses a release but shows a later version", async () => {
    render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss Codex update" }));
    expect(screen.queryByTestId("provider-update-notice")).not.toBeInTheDocument();
    const key = updateTargetKey({ provider: "codex" });
    act(() => useProviderUpdates.setState(s => ({ slots: { [key]: { ...s.slots[key], report: { ...report, latest_version: "0.159.0" } } } })));
    expect(await screen.findByText(/0.159.0/)).toBeVisible();
  });
  it("offers guidance for external installs", async () => {
    invoke.mockResolvedValue({ ...report, can_update: false, manager: "External installation", message: "Use your package manager." });
    render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    expect(await screen.findByRole("link", { name: /Update guide/ })).toBeVisible();
    expect(screen.queryByRole("button", { name: "Update" })).not.toBeInTheDocument();
  });
  it("checks hourly and cleans up its timer", async () => {
    vi.useFakeTimers();
    const { unmount } = render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    await act(async () => {});
    await act(async () => vi.advanceTimersByTime(UPDATE_INTERVAL));
    expect(invoke).toHaveBeenCalledTimes(2);
    unmount();
    await act(async () => vi.advanceTimersByTime(UPDATE_INTERVAL));
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("stays quiet when no update exists", async () => {
    invoke.mockResolvedValue({ ...report, available: false });
    render(<ProviderUpdateNotice provider="codex" threadId="demo" />);
    await waitFor(() => expect(invoke).toHaveBeenCalledOnce());
    expect(screen.queryByTestId("provider-update-notice")).not.toBeInTheDocument();
  });
  it("does not offer local maintenance in a remote workspace", () => {
    render(<ProviderUpdateNotice provider="codex" threadId="demo" remote />);
    expect(invoke).not.toHaveBeenCalled();
    expect(screen.queryByTestId("provider-update-notice")).not.toBeInTheDocument();
  });

});
