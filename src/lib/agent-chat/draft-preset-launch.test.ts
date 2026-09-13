import { beforeEach, expect, it, vi } from "vitest";

vi.mock("./materialize", () => ({ materializeWithPreset: vi.fn() }));
vi.mock("./skill-selection-refresh", () => ({ refreshSkillSelectionForCwd: vi.fn() }));
vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/tauri/commands")>(),
  listSkills: vi.fn(),
}));

import { launchDraftWithPreset } from "./draft-preset-launch";
import { materializeWithPreset } from "./materialize";
import { refreshSkillSelectionForCwd } from "./skill-selection-refresh";
import { listSkills, type Skill } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore, type DraftTarget } from "@/stores/chat-draft-store";
import { useSkillsStore } from "@/stores/skills-store";
import type { AppStateSnapshot, TerminalPreset } from "@/tauri/types";

beforeEach(() => {
  vi.mocked(materializeWithPreset).mockReset().mockResolvedValue({ success: false, error: "test" });
  vi.mocked(refreshSkillSelectionForCwd).mockReset();
  vi.mocked(listSkills).mockReset();
  useChatDraftStore.setState({ draftsById: {}, activeHomeDraftId: null, activeDraftId: null });
  useSkillsStore.setState({
    skills: [], disabledIds: [], includePlugins: true, inventoryCache: {},
    inFlightContexts: {}, activeContextKey: null, cacheGeneration: 0,
  });
  useAppStore.setState({
    homeDir: "/home/user",
    appState: { workspaces: [{ workspace_id: "ws", cwd: "/worktree" }] } as AppStateSnapshot,
  });
});

it.each<[DraftTarget, string]>([
  [{ kind: "home" }, "/home/user"],
  [{ kind: "project", projectPath: "/project" }, "/project"],
  [{ kind: "existing_workspace", workspaceId: "ws" }, "/worktree"],
])("launches a preset with the selected draft's skills for %j", async (target, cwd) => {
  const selected = { id: "selected-id", name: "review", provider: "claude", scope: "user" } as Skill;
  const unrelated = { ...selected, id: "unrelated-id" };
  vi.mocked(listSkills)
    .mockResolvedValueOnce({ skills: [selected], errors: [] })
    .mockResolvedValueOnce({ skills: [unrelated], errors: [] });
  await useSkillsStore.getState().loadSkills(cwd);
  // A different mounted composer loaded another same-named skill afterward.
  await useSkillsStore.getState().loadSkills("/unrelated");
  const store = useChatDraftStore.getState();
  const draft = store.getOrCreateHomeDraft();
  store.updateDraftTarget(draft.draftId, target);
  store.updateDraftConfig(draft.draftId, { provider: "claude" });
  store.updateDraftInput(draft.draftId, "/review check this");

  await launchDraftWithPreset(draft.draftId, { id: "chat" } as TerminalPreset);
  const call = vi.mocked(materializeWithPreset).mock.calls[0];
  const selection = { skillIds: ["selected-id"], text: "check this" };
  expect(call[4]).toEqual(selection);
  await call[3].refreshSkillSelection!(selection, "/new-worktree");
  expect(refreshSkillSelectionForCwd).toHaveBeenCalledWith(
    selection, [selected], cwd, "/new-worktree", "claude",
  );
});
