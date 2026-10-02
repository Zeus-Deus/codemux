/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, act } from "@testing-library/react";
import { agentChatHooks, type NativeHook, type NativeHooksList } from "@/tauri/commands";
import { HooksManager } from "./HooksManager";

vi.mock("@/tauri/commands", () => ({ agentChatHooks: vi.fn() }));
const api = vi.mocked(agentChatHooks);
const hook: NativeHook = {
  key: "path:/repo/.codex/hooks.json:Stop:0", currentHash: "current-version",
  eventName: "stop", handlerType: "command", command: "npm run lint",
  source: "project", sourcePath: "/repo/.codex/hooks.json", enabled: false,
  isManaged: false, trustStatus: "untrusted", timeoutSec: 30,
};
const catalogue = (hooks = [hook], cwd = "/repo"): NativeHooksList => ({ cwd, hooks, warnings: [], errors: [] });
const props = { open: true, onOpenChange: vi.fn(), cwd: "/repo", threadId: "thread" };

describe("Codex hook management", () => {
  beforeEach(() => { api.mockReset(); api.mockResolvedValue(catalogue()); });
  afterEach(cleanup);

  it("reviews the exact definition before trusting and enabling through native config", async () => {
    render(<HooksManager {...props} />);
    expect(await screen.findByText("npm run lint")).toBeVisible();
    expect(api).toHaveBeenCalledWith("codex", "/repo", "thread");
    expect(screen.getByRole("button", { name: "Enable hook" })).toBeDisabled();
    api.mockResolvedValueOnce(catalogue([{ ...hook, trustStatus: "trusted" }]));
    fireEvent.click(screen.getByRole("button", { name: "Trust this hook" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Enable hook" })).toBeEnabled());
    expect(api).toHaveBeenLastCalledWith("codex", "/repo", "thread", { action: "trust", key: hook.key, hash: "current-version" });
    api.mockResolvedValueOnce(catalogue([{ ...hook, trustStatus: "trusted", enabled: true }]));
    fireEvent.click(screen.getByRole("button", { name: "Enable hook" }));
    expect(await screen.findByRole("button", { name: "Disable hook" })).toBeEnabled();
    expect(api).toHaveBeenLastCalledWith("codex", "/repo", "thread", { action: "setEnabled", key: hook.key, hash: "current-version", enabled: true });
  });

  it("keeps managed hooks read-only and shows MCP tool definitions", async () => {
    api.mockResolvedValue(catalogue([{ ...hook, isManaged: true, trustStatus: "managed", handlerType: "mcpTool", command: undefined, server: "checks", tool: "lint" }]));
    render(<HooksManager {...props} />);
    expect(await screen.findByText("checks/lint")).toBeVisible();
    expect(screen.getByText("This hook is managed by your administrator.")).toBeVisible();
    expect(screen.queryByRole("button", { name: /Trust this hook|Enable hook|Disable hook/ })).toBeNull();
  });

  it("surfaces native errors and refreshes instead of pretending an update succeeded", async () => {
    render(<HooksManager {...props} />);
    await screen.findByText("npm run lint");
    api.mockRejectedValueOnce("This hook changed since you reviewed it.");
    fireEvent.click(screen.getByRole("button", { name: "Trust this hook" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This hook changed");
    expect(screen.getByRole("button", { name: "Enable hook" })).toBeDisabled();
    api.mockResolvedValueOnce(catalogue([{ ...hook, currentHash: "new-version", command: "npm run check" }]));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("npm run check");
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Trust this hook" }));
    expect(api).toHaveBeenLastCalledWith("codex", "/repo", "thread", { action: "trust", key: hook.key, hash: "new-version" });
  });

  it("ignores old responses after switching project and does not probe a closed dialog", async () => {
    let resolveOld!: (value: NativeHooksList) => void;
    api.mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }));
    const view = render(<HooksManager {...props} open={false} />);
    expect(api).not.toHaveBeenCalled();
    view.rerender(<HooksManager {...props} />);
    api.mockResolvedValueOnce(catalogue([], "/other"));
    view.rerender(<HooksManager {...props} cwd="/other" threadId={null} />);
    await screen.findByText(/No lifecycle hooks found/);
    await act(async () => { resolveOld(catalogue()); });
    expect(screen.queryByText("npm run lint")).toBeNull();
    expect(api).toHaveBeenLastCalledWith("codex", "/other", null);
  });

  it("retries discovery failure and shows native configuration problems", async () => {
    api.mockRejectedValueOnce("Codex hook discovery failed: unsupported method");
    render(<HooksManager {...props} cwd={null} threadId={null} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("unsupported method");
    api.mockResolvedValueOnce({ ...catalogue([]), warnings: ["Plugin skipped"], errors: [{ path: "hooks.json", message: "Invalid matcher" }] });
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await screen.findByText("Plugin skipped");
    expect(screen.getByRole("alert")).toHaveTextContent("Invalid matcher");
    expect(api).toHaveBeenLastCalledWith("codex", null, null);
  });
});
