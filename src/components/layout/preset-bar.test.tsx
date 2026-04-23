/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { PresetStoreSnapshot, TerminalPreset } from "@/tauri/types";

const mockGetPresets = vi.fn();
const mockApplyPreset = vi.fn();
const mockSetPresetBarVisible = vi.fn().mockResolvedValue(undefined);
// RunButton reaches for these. Return null/empty so the child renders its
// "Set Run" empty state without hitting any real IPC.
const mockGetProjectScripts = vi.fn().mockResolvedValue(null);
const mockGetWorkspaceConfig = vi.fn().mockResolvedValue(null);
const mockRunProjectDevCommand = vi.fn().mockResolvedValue(undefined);

const mockAgentChatCreatePane = vi.fn().mockResolvedValue("pane-ca");
const mockMaterializeWithPreset = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getPresets: (...args: unknown[]) => mockGetPresets(...args),
  applyPreset: (...args: unknown[]) => mockApplyPreset(...args),
  setPresetBarVisible: (...args: unknown[]) => mockSetPresetBarVisible(...args),
  getProjectScripts: (...args: unknown[]) => mockGetProjectScripts(...args),
  getWorkspaceConfig: (...args: unknown[]) => mockGetWorkspaceConfig(...args),
  runProjectDevCommand: (...args: unknown[]) => mockRunProjectDevCommand(...args),
  agentChatCreatePane: (...args: unknown[]) => mockAgentChatCreatePane(...args),
}));

vi.mock("@/lib/agent-chat/materialize", () => ({
  materializeWithPreset: (...args: unknown[]) => mockMaterializeWithPreset(...args),
}));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: () => Promise.resolve(() => {}),
}));

vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

const mockSetShowSettings = vi.fn();

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
      return selector({ setShowSettings: mockSetShowSettings });
    }),
    {
      getState: () => ({ setShowSettings: mockSetShowSettings }),
    },
  ),
}));

// RunButton pulls its own dependencies (getProjectScripts / getWorkspaceConfig
// / useActiveWorkspace). Mock the parts that would otherwise hit unmocked
// modules so we can mount <PresetBar> without dragging in every store.
vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => ({
    workspace_id: "ws-1",
    project_root: "/home/user/myapp",
  }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { PresetBar } from "./preset-bar";
import { toast as mockToast } from "@/lib/toast";
import {
  useChatDraftStore,
  type DraftId,
} from "@/stores/chat-draft-store";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

function makePreset(overrides: Partial<TerminalPreset> = {}): TerminalPreset {
  return {
    id: "claude-code",
    name: "Claude Code",
    description: null,
    commands: ["claude"],
    working_directory: null,
    launch_mode: "new_tab",
    icon: null,
    pinned: true,
    is_builtin: true,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    ...overrides,
  };
}

function makeSnapshot(presets: TerminalPreset[]): PresetStoreSnapshot {
  return {
    presets,
    bar_visible: true,
    default_preset_id: null,
  };
}

function renderPresetBar(workspaceId = "ws-1") {
  return render(
    <TooltipProvider>
      <PresetBar workspaceId={workspaceId} />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("PresetBar — preset failure feedback", () => {
  it("shows an error toast when applyPreset rejects with 'X is not installed'", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    // Matches the exact shape the Rust backend returns: a plain string
    // from `Err(format!("{} is not installed", binary))`.
    mockApplyPreset.mockRejectedValue("claude is not installed");

    renderPresetBar();
    await flushPromises();

    const button = screen.getByRole("button", { name: /claude code/i });
    await userEvent.click(button);
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledWith("ws-1", "claude-code", undefined);
    expect(mockToast.error).toHaveBeenCalledTimes(1);
    expect(mockToast.error).toHaveBeenCalledWith(
      "Claude Code: claude is not installed",
    );
  });

  it("handles Error objects (not just strings) gracefully", async () => {
    mockGetPresets.mockResolvedValue(
      makeSnapshot([makePreset({ id: "codex", name: "Codex", commands: ["codex"] })]),
    );
    mockApplyPreset.mockRejectedValue(new Error("codex is not installed"));

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /codex/i }));
    await flushPromises();

    expect(mockToast.error).toHaveBeenCalledWith("Codex: codex is not installed");
  });

  it("falls back to String(err) for unknown rejection shapes", async () => {
    mockGetPresets.mockResolvedValue(
      makeSnapshot([makePreset({ id: "aider", name: "Aider", commands: ["aider"] })]),
    );
    // Something weird — not a string, not an Error. The catch should still
    // produce a human-readable toast instead of showing `[object Object]`
    // unless `String()` would naturally produce that. The test's purpose
    // here is "it doesn't throw and it does call toast.error once".
    mockApplyPreset.mockRejectedValue({ weird: "shape" });

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /aider/i }));
    await flushPromises();

    expect(mockToast.error).toHaveBeenCalledTimes(1);
    // We don't assert the exact message here — just that the fallback
    // executed without throwing and the preset name is prefixed. Use
    // `vi.mocked()` to tell TypeScript this is a Mock (the real type
    // signature of `toast.error` is `(msg, opts?) => string | number`).
    const call = vi.mocked(mockToast.error).mock.calls[0]?.[0] as string;
    expect(call).toMatch(/^Aider: /);
  });

  it("does NOT show a toast on successful apply", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    mockApplyPreset.mockResolvedValue(undefined);

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /claude code/i }));
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledTimes(1);
    expect(mockToast.error).not.toHaveBeenCalled();
  });

  it("passes 'split_pane' mode when user shift-clicks the preset", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    mockApplyPreset.mockResolvedValue(undefined);

    renderPresetBar();
    await flushPromises();

    // userEvent v14's shift-modifier syntax (`{Shift>}...{/Shift}`) does not
    // reliably set `MouseEvent.shiftKey` on the synthetic click event, so we
    // use `fireEvent.click` with an explicit `shiftKey: true` init to match
    // what the real user interaction produces in the browser.
    const button = screen.getByRole("button", { name: /claude code/i });
    fireEvent.click(button, { shiftKey: true });
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledWith("ws-1", "claude-code", "split_pane");
  });
});

// ── Draft mode + CLI-on-Home gating (Task 7 + Task 7b) ──

function resetDraftStore() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
}

function renderDraftPresetBar(options: {
  presets: TerminalPreset[];
  draftId: DraftId;
  disabled?: boolean;
}) {
  return render(
    // `delayDuration={0}` short-circuits Radix's default hover delay
    // so `userEvent.hover` + `findByText` can settle within the test
    // timeout. Matches the pattern used elsewhere in the app-shell
    // overlay tests.
    <TooltipProvider delayDuration={0}>
      <PresetBar
        workspaceId={null}
        draftId={options.draftId}
        disabled={options.disabled}
      />
    </TooltipProvider>,
  );
}

describe("PresetBar — draft mode dispatch", () => {
  beforeEach(() => {
    resetDraftStore();
    mockMaterializeWithPreset.mockReset();
    mockMaterializeWithPreset.mockResolvedValue({
      success: true,
      workspaceId: "ws-new",
      paneId: "pane-new",
      threadId: "tid-new",
    });
  });

  it("draft mode + click routes through materializeWithPreset with the draft's composer text", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore
      .getState()
      .updateDraftTarget(draft.draftId, {
        kind: "project",
        projectPath: "/p",
      });
    useChatDraftStore.getState().updateDraftInput(draft.draftId, "type this");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const chatAgent = makePreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([chatAgent]));

    renderDraftPresetBar({ presets: [chatAgent], draftId: draft.draftId });
    await flushPromises();

    const button = screen.getByRole("button", { name: /chat agent/i });
    await userEvent.click(button);
    await flushPromises();

    expect(mockMaterializeWithPreset).toHaveBeenCalledTimes(1);
    const [passedDraft, passedPreset, passedPrompt] =
      mockMaterializeWithPreset.mock.calls[0];
    expect(passedDraft.draftId).toBe(draft.draftId);
    expect(passedPreset.id).toBe("builtin-chat-agent");
    expect(passedPrompt).toBe("type this");
    // Workspace-mode dispatch must not fire in draft mode.
    expect(mockApplyPreset).not.toHaveBeenCalled();
    expect(mockAgentChatCreatePane).not.toHaveBeenCalled();
  });

  it("on successful materialize the active draft is cleared", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore
      .getState()
      .updateDraftTarget(draft.draftId, {
        kind: "project",
        projectPath: "/p",
      });
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const chatAgent = makePreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([chatAgent]));

    renderDraftPresetBar({ presets: [chatAgent], draftId: draft.draftId });
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /chat agent/i }));
    await flushPromises();

    expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  });
});

describe("PresetBar — Home-draft full gating (Task 7b revised)", () => {
  beforeEach(() => {
    resetDraftStore();
    mockMaterializeWithPreset.mockReset();
    mockMaterializeWithPreset.mockResolvedValue({
      success: true,
      workspaceId: "ws-new",
      paneId: "pane-new",
      threadId: "tid-new",
    });
  });

  it("Home draft + CLI preset → button is disabled and click is a no-op", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const cli = makePreset({ kind: "cli" });
    mockGetPresets.mockResolvedValue(makeSnapshot([cli]));

    renderDraftPresetBar({ presets: [cli], draftId: draft.draftId });
    await flushPromises();

    const button = screen.getByRole("button", { name: /claude code/i });
    expect(button).toBeDisabled();
    // userEvent refuses to click disabled controls by default, matching
    // the real browser semantics. Firing the low-level event bypasses
    // pointer-capture checks and confirms our handler still bails.
    fireEvent.click(button);
    await flushPromises();
    expect(mockMaterializeWithPreset).not.toHaveBeenCalled();
    expect(mockApplyPreset).not.toHaveBeenCalled();
  });

  it("Home draft + Chat Agent preset → button is ALSO disabled (prevents duplicate chat)", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const chatAgent = makePreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([chatAgent]));

    renderDraftPresetBar({ presets: [chatAgent], draftId: draft.draftId });
    await flushPromises();

    const button = screen.getByRole("button", { name: /chat agent/i });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    await flushPromises();
    // No dispatch of any kind — the user is already in a chat draft.
    expect(mockMaterializeWithPreset).not.toHaveBeenCalled();
    expect(mockApplyPreset).not.toHaveBeenCalled();
    expect(mockAgentChatCreatePane).not.toHaveBeenCalled();
  });

  it("Project draft + Chat Agent preset → enabled (regression)", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/projects/foo");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const chatAgent = makePreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([chatAgent]));

    renderDraftPresetBar({ presets: [chatAgent], draftId: draft.draftId });
    await flushPromises();

    const button = screen.getByRole("button", { name: /chat agent/i });
    expect(button).not.toBeDisabled();
    await userEvent.click(button);
    await flushPromises();

    expect(mockMaterializeWithPreset).toHaveBeenCalledTimes(1);
  });

  it("Project draft + CLI preset → button is enabled", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/projects/foo");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const cli = makePreset({ kind: "cli" });
    mockGetPresets.mockResolvedValue(makeSnapshot([cli]));

    renderDraftPresetBar({ presets: [cli], draftId: draft.draftId });
    await flushPromises();

    expect(screen.getByRole("button", { name: /claude code/i })).not.toBeDisabled();
  });

  it("disabled prop (materializing) → every preset button is disabled", async () => {
    const draft = useChatDraftStore
      .getState()
      .getOrCreateProjectDraft("/projects/foo");
    useChatDraftStore.getState().setActiveDraft(draft.draftId);

    const cli = makePreset({ id: "cli-1", name: "Claude Code", kind: "cli" });
    const chat = makePreset({
      id: "chat-1",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([cli, chat]));

    renderDraftPresetBar({
      presets: [cli, chat],
      draftId: draft.draftId,
      disabled: true,
    });
    await flushPromises();

    expect(screen.getByRole("button", { name: /claude code/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /chat agent/i })).toBeDisabled();
  });
});

describe("PresetBar — ChatAgent on a real workspace", () => {
  beforeEach(() => {
    resetDraftStore();
    mockAgentChatCreatePane.mockClear().mockResolvedValue("pane-ca");
  });

  it("workspace-mode click on Chat Agent spawns a sibling agent_chat pane", async () => {
    const chatAgent = makePreset({
      id: "builtin-chat-agent",
      name: "Chat Agent",
      kind: "chat_agent",
      commands: [],
    });
    mockGetPresets.mockResolvedValue(makeSnapshot([chatAgent]));

    renderPresetBar("ws-real");
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /chat agent/i }));
    await flushPromises();

    expect(mockAgentChatCreatePane).toHaveBeenCalledWith(
      "ws-real",
      "claude",
      null,
    );
    // The CLI dispatch path must not fire for a ChatAgent preset.
    expect(mockApplyPreset).not.toHaveBeenCalled();
  });
});
