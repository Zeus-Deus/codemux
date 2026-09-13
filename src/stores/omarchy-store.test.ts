import { beforeEach, expect, it, vi } from "vitest";
import { fallbackTheme } from "@/hooks/use-theme-colors";
import type { OmarchyTheme } from "@/lib/omarchy-theme";

const api = vi.hoisted(() => ({ read: vi.fn(), listen: vi.fn(), remote: false }));
vi.mock("@/tauri/omarchy", () => ({ getOmarchyTheme: api.read, onOmarchyThemeChanged: api.listen }));
vi.mock("@/components/remote/is-remote-client", () => ({ isRemoteClient: () => api.remote }));
const palette: OmarchyTheme = { name: "Tokyo Night", scheme: "dark", colors: fallbackTheme };
beforeEach(() => { vi.resetModules(); api.read.mockReset(); api.listen.mockReset(); api.remote = false; });

it("subscribes before reading and never lets a stale startup read replace a newer desktop palette", async () => {
  let receive!: (theme: OmarchyTheme) => void;
  let resolve!: (theme: OmarchyTheme) => void;
  api.listen.mockImplementation(async (callback) => { receive = callback; return () => {}; });
  api.read.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const { useOmarchyStore } = await import("./omarchy-store");
  const loading = useOmarchyStore.getState().load();
  await vi.waitFor(() => expect(api.read).toHaveBeenCalled());
  const latest = { ...palette, name: "Catppuccin" };
  receive(latest);
  resolve(palette);
  await loading;
  expect(useOmarchyStore.getState().theme?.label).toBe("Omarchy · Catppuccin");
  expect(useOmarchyStore.getState().loaded).toBe(true);
});

it("does not discover or subscribe to the server's desktop from a remote browser", async () => {
  api.remote = true;
  const { useOmarchyStore } = await import("./omarchy-store");
  await useOmarchyStore.getState().load();
  expect(api.read).not.toHaveBeenCalled();
  expect(api.listen).not.toHaveBeenCalled();
  expect(useOmarchyStore.getState()).toMatchObject({ loaded: true, theme: null });
});

it("keeps absence distinct from a fallback palette and loads only once", async () => {
  api.listen.mockResolvedValue(() => {});
  api.read.mockResolvedValue(null);
  const { useOmarchyStore } = await import("./omarchy-store");
  await Promise.all([useOmarchyStore.getState().load(), useOmarchyStore.getState().load()]);
  expect(api.read).toHaveBeenCalledTimes(1);
  expect(useOmarchyStore.getState()).toMatchObject({ loaded: true, theme: null });
});
