/// <reference types="@testing-library/jest-dom/vitest" />
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { registry } from "@/contrib/registry";
import { TITLEBAR_ACTIONS_AREA } from "@/sdk/areas";
import { WindowChrome } from "./window-chrome";

const state = vi.hoisted(() => ({ remote: false }));
vi.mock("@/hooks/use-mobile-layout", () => ({ useMobileLayout: () => false }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => state.remote }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ isMaximized: async () => false, onResized: async () => () => {} }),
}));
afterEach(() => { cleanup(); state.remote = false; });

it("keeps standalone Settings chrome unchanged without contributions", () => {
  render(<WindowChrome />);
  expect(screen.getAllByRole("button")).toHaveLength(3);
  expect(document.querySelector("[data-plugin-titlebar-actions]")).toBeNull();
});

it.each([false, true])("mounts plugin actions in standalone chrome (remote=%s)", (remote) => {
  state.remote = remote;
  const off = registry.register({
    id: "demo:safety", area: TITLEBAR_ACTIONS_AREA,
    render: () => <button type="button">Stop plugin control</button>,
  });
  try {
    render(<WindowChrome />);
    expect(screen.getByRole("button", { name: "Stop plugin control" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Minimize" }) !== null).toBe(!remote);
  } finally { cleanup(); off(); }
});
