import { beforeAll, expect, it } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { AppStateSnapshot } from "@/tauri/types";

beforeAll(async () => {
  await import("./tauri-mock");
});

it("publishes immutable pin snapshots and preserves a pin's timestamp", async () => {
  const before = await invoke<AppStateSnapshot>("get_app_state");
  const id = "ws-followup-folds";
  const original = before.workspaces.find(
    (workspace) => workspace.workspace_id === id,
  );
  expect(original?.pinned_at ?? null).toBeNull();
  await invoke("set_workspace_pinned", { workspaceId: id, pinned: true });
  const pinned = await invoke<AppStateSnapshot>("get_app_state");
  expect(pinned.workspaces).not.toBe(before.workspaces);
  const timestamp = pinned.workspaces.find(
    (workspace) => workspace.workspace_id === id,
  )?.pinned_at;
  expect(timestamp).toEqual(expect.any(Number));
  expect(original?.pinned_at ?? null).toBeNull();
  await invoke("set_workspace_pinned", { workspaceId: id, pinned: true });
  expect(
    (await invoke<AppStateSnapshot>("get_app_state")).workspaces.find(
      (workspace) => workspace.workspace_id === id,
    )?.pinned_at,
  ).toBe(timestamp);
  await invoke("set_workspace_pinned", { workspaceId: id, pinned: false });
  expect(
    (await invoke<AppStateSnapshot>("get_app_state")).workspaces.find(
      (workspace) => workspace.workspace_id === id,
    )?.pinned_at,
  ).toBeNull();
  expect(
    pinned.workspaces.find((workspace) => workspace.workspace_id === id)
      ?.pinned_at,
  ).toBe(timestamp);
});

it("rejects a vanished workspace like the pin command does", async () => {
  await expect(
    invoke("set_workspace_pinned", { workspaceId: "removed", pinned: true }),
  ).rejects.toThrow("No workspace found");
});
