import { invoke } from "@tauri-apps/api/core";

/** Public metadata only. OAuth credentials never cross the native boundary. */
export interface ChatGptStatus {
  phase: "disconnected" | "pending" | "connected" | "error";
  attemptId: string | null;
  email: string | null;
  error: string | null;
  profiles: { id: string; label: string; email: string | null; connected: boolean }[];
  activeProfileId: string | null;
  welcomePending: boolean;
  installed: boolean;
}
export const getChatGptStatus = () => invoke<ChatGptStatus>("get_chatgpt_status");
export const startChatGptLogin = (profileId: string | null) =>
  invoke<ChatGptStatus>("start_chatgpt_login", { profileId });
export const cancelChatGptLogin = (attemptId: string) =>
  invoke<ChatGptStatus>("cancel_chatgpt_login", { attemptId });
export const disconnectChatGpt = () => invoke<ChatGptStatus>("disconnect_chatgpt");
export const acknowledgeChatGptWelcome = () => invoke<ChatGptStatus>("acknowledge_chatgpt_welcome");
export const getLocalWorkbench = () => invoke<boolean>("get_local_workbench");
export const setLocalWorkbench = (enabled: boolean) => invoke<void>("set_local_workbench", { enabled });
/** Paired browsers only: true when a debug-build host runs the live test
 *  harness (`CODEMUX_E2E_LOCAL_REMOTE=1`). Always false on release hosts. */
export const getE2eLocalRemote = () => invoke<boolean>("web_remote_e2e_local_mode");
