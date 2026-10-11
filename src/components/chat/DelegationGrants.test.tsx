import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { grantFixture } from "@/lib/delegation.test-fixtures";
import { useDelegationTasks } from "@/stores/delegation-store";
const native = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: native.listen }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });

it("allows workspace access to be revoked offline and confirms the exact target before showing success", async () => {
  const { DelegationGrants } = await import("./DelegationGrants");
  let revoked = false;
  native.invoke.mockImplementation(command => {
    if (command === "delegation_grants") return Promise.resolve([{ ...grantFixture, enabled: !revoked }]);
    if (command === "delegation_revoke") { revoked = true; return Promise.resolve(); }
    return Promise.resolve([]);
  });
  renderHook(() => useDelegationTasks("pane", "thread-1"));
  render(<DelegationGrants paneId="pane" threadId="thread-1" writable />);
  fireEvent.click(screen.getByText("Authorized targets in this workspace"));
  // jsdom does not implement the details toggle default action.
  const details = screen.getByText("Authorized targets in this workspace").closest("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  await screen.findByRole("button", { name: "Revoke Build host access" });
  fireEvent.click(screen.getByRole("button", { name: "Revoke Build host access" }));
  await waitFor(() => expect(screen.getByText("Revoked")).toBeInTheDocument());
  expect(native.invoke).toHaveBeenCalledWith("delegation_revoke", { paneId: "pane", targetId: "grant-1" });
  expect(native.invoke.mock.calls.some(([command]) => command === "delegation_host_info" || command === "delegate_task")).toBe(false);
});
