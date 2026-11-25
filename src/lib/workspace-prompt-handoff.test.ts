import { beforeEach, expect, it, vi } from "vitest";

vi.mock("@/lib/clipboard", () => ({
  copyToClipboard: vi.fn().mockResolvedValue(true),
  COPY_FAILED_MESSAGE: "Couldn't copy",
}));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

import { copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { saveWorkspacePrompt } from "./workspace-prompt-handoff";

beforeEach(() => {
  vi.clearAllMocks();
  useChatDraftStore.setState({ draftsById: {}, activeDraftId: null });
});

it("saves and copies the exact prepared prompt", async () => {
  const prompt = "Review #92\n\nDescription:\nRun `$(date)` literally";
  saveWorkspacePrompt("ws-92", prompt, true, "Launch command reached a terminal");

  const saved = Object.values(useChatDraftStore.getState().draftsById)[0];
  expect(saved.inputDraft).toBe(prompt);
  expect(saved.target).toEqual({ kind: "existing_workspace", workspaceId: "ws-92" });

  const options = vi.mocked(toast.success).mock.calls[0][1];
  expect(options?.action).toMatchObject({ label: "Copy prompt" });
  expect(options?.description).toContain("Paste it only when the agent's input is visible");
  const action = options?.action;
  if (!action || typeof action !== "object" || !("onClick" in action)) {
    throw new Error("Copy action missing");
  }
  const preventDefault = vi.fn();
  action.onClick({ preventDefault } as never);
  await vi.waitFor(() => expect(copyToClipboard).toHaveBeenCalledWith(prompt));
  expect(preventDefault).toHaveBeenCalledOnce();
});

it("warns and retains the prompt when a terminal command was not confirmed", () => {
  saveWorkspacePrompt("ws-92", "Fix the app", false, "Agent could not start", "warning");

  const saved = Object.values(useChatDraftStore.getState().draftsById)[0];
  expect(saved.inputDraft).toBe("Fix the app");
  expect(saved.manualPromptRequired).toBe(false);
  expect(toast.warning).toHaveBeenCalledWith(
    "Agent could not start",
    expect.objectContaining({ action: expect.objectContaining({ label: "Copy prompt" }) }),
  );
  expect(vi.mocked(toast.warning).mock.calls[0][1]?.description).toBe("The exact prompt is saved in Drafts.");
  expect(toast.success).not.toHaveBeenCalled();
});

it("offers to reopen a confirmed terminal after a partial launch", async () => {
  const openTerminal = vi.fn().mockResolvedValue(undefined);
  saveWorkspacePrompt("ws-92", "Fix the app", true, "One of two tabs opened", "warning", openTerminal);

  const saved = Object.values(useChatDraftStore.getState().draftsById)[0];
  expect(saved.inputDraft).toBe("Fix the app");
  expect(saved.manualPromptRequired).toBe(true);
  const options = vi.mocked(toast.warning).mock.calls[0][1];
  expect(options?.action).toMatchObject({ label: "Copy prompt" });
  expect(options?.description).toContain("Paste it only when the agent's input is visible");
  expect(options?.cancel).toMatchObject({ label: "Open terminal" });
  const cancel = options?.cancel;
  if (!cancel || typeof cancel !== "object" || !("onClick" in cancel)) {
    throw new Error("Open terminal action missing");
  }
  const preventDefault = vi.fn();
  cancel.onClick({ preventDefault } as never);
  await vi.waitFor(() => expect(openTerminal).toHaveBeenCalledOnce());
  expect(preventDefault).toHaveBeenCalledOnce();
  expect(toast.success).not.toHaveBeenCalled();
});
