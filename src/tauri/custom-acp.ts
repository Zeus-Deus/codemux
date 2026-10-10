import type { ProviderChatCapabilities } from "./types";
import { invoke } from "@tauri-apps/api/core";

export interface AcpAgent {
  id: string;
  name: string;
  executable: string;
  args: string[];
  /** Listings redact values. Null in an update retains the saved value. */
  environment: Record<string, string | null>;
  enabled: boolean;
  auth_method: string | null;
  revision: string;
}
export type AcpAgentInput = Omit<AcpAgent, "id" | "revision"> & { id?: string };
export interface AcpConfigOption {
  id: string;
  name: string;
  description: string | null;
  category: string | null;
  type: "select" | "boolean";
  current_value: string | boolean;
  options: { value: string; name: string; description: string | null }[];
}
export interface AcpAuthMethod { id: string; name: string; description: string | null; type: string }
export interface AcpCatalog {
  agent_id: string;
  agent_name: string;
  capabilities: ProviderChatCapabilities;
  config_options: AcpConfigOption[];
  current_model: string | null;
  supports_resume: boolean;
  supports_images: boolean;
  auth_methods: AcpAuthMethod[];
}
export interface AcpThreadCatalog {
  catalog: AcpCatalog;
  /** Current native owner status, read atomically with its catalog; never durable. */
  live: boolean;
}
export interface AcpBinding {
  thread_id: string;
  agent_id: string;
  revision: string;
  cwd: string;
  session_id: string | null;
  catalog: AcpCatalog;
  config_values: Record<string, string | boolean>;
}
export const acpAgents = () => invoke<AcpAgent[]>("acp_agents");
export const acpSaveAgent = (input: AcpAgentInput) => invoke<AcpAgent>("acp_save_agent", { input });
export const acpDeleteAgent = (agentId: string) => invoke<void>("acp_delete_agent", { agentId });
export const acpProbe = (agentId: string, cwd: string | null = null) => invoke<AcpCatalog>("acp_probe", { agentId, cwd });
export const acpBinding = (threadId: string) => invoke<AcpBinding | null>("acp_binding", { threadId });
export const acpThreadCatalog = (threadId: string) => invoke<AcpThreadCatalog>("acp_thread_catalog", { threadId });
export const acpCatalog = (threadId: string) => invoke<AcpCatalog>("acp_catalog", { threadId });
export const acpSetConfig = (threadId: string, configId: string, value: string | boolean) => invoke<AcpCatalog>("acp_set_config", { threadId, configId, value });
