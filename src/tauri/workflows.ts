import { randomUUID } from "@/lib/uuid";
import { invoke } from "@tauri-apps/api/core";
import type { AgentChatProviderKind } from "./types";

export interface WorkflowRoute {
  id: string;
  provider: AgentChatProviderKind;
  model?: string | null;
  effort?: string | null;
}

export interface WorkflowLimits {
  concurrency: number;
  max_tasks: number;
  max_attempts: number;
  max_depth: number;
  token_budget?: number | null;
  max_output_bytes: number;
  wall_time_ms: number;
}

export interface WorkflowTaskSpec {
  id: string;
  title: string;
  prompt: string;
  dependencies: string[];
  route_id?: string | null;
  access: "read_only" | "write";
  scope: string[];
  output_schema?: unknown;
  required: boolean;
}

export interface WorkflowRunSpec {
  workspace_id: string;
  title: string;
  goal: string;
  mode: "dry_run" | "live";
  allow_writes: boolean;
  routes: WorkflowRoute[];
  limits: WorkflowLimits;
  tasks: WorkflowTaskSpec[];
  script?: { source: string; args: unknown; api_version: 1 } | null;
}

export type WorkflowRunStatus = "running" | "paused" | "completed" | "failed" | "cancelled" | "stopping" | "unknown";
export type WorkflowTaskStatus = "queued" | "running" | "waiting" | "succeeded" | "failed" | "cancelled" | "blocked" | "unknown" | "stopping";

export interface WorkflowUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  reserved_tokens: number;
  estimated_tokens: number;
  tokens_unknown: boolean;
  cost_usd: number | null;
  cost_unknown: boolean;
}

export interface WorkflowAttempt {
  id: string;
  generation: number;
  operation_id: string;
  status: "dispatching" | "running" | "succeeded" | "failed" | "cancelled" | "waiting" | "unknown" | "stopping";
  route_id: string;
  started_at_ms: number;
  finished_at_ms: number | null;
  cancel_requested: boolean;
  reserved_tokens: number;
  external_ref: unknown;
  output: unknown;
  error: string | null;
  usage: WorkflowUsage;
  artifacts: unknown[];
}

export interface WorkflowTaskSnapshot {
  retired: boolean;
  spec: WorkflowTaskSpec;
  generation: number;
  status: WorkflowTaskStatus;
  depth: number;
  parent_task_id: string | null;
  current_attempt: WorkflowAttempt | null;
  attempts: WorkflowAttempt[];
  result: unknown;
  error: string | null;
  waiting_for: string[];
  messages: string[];
}

export interface WorkflowRunSnapshot {
  id: string;
  status: WorkflowRunStatus;
  cancel_requested: boolean;
  pause_requested: boolean;
  error: string | null;
  revision: number;
  created_at_ms: number;
  updated_at_ms: number;
  spec: WorkflowRunSpec;
  tasks: WorkflowTaskSnapshot[];
  usage: WorkflowUsage;
  resolved_limits: WorkflowLimits;
  script?: { status: "pending" | "running" | "paused" | "completed" | "failed"; result: unknown; error: string | null; phase: string | null } | null;
}

export interface WorkflowRunSummary {
  id: string;
  status: WorkflowRunStatus;
  revision: number;
  created_at_ms: number;
  updated_at_ms: number;
  spec: { workspace_id: string; title: string; mode: "dry_run" | "live" };
}

export interface WorkflowCapability {
  provider: AgentChatProviderKind;
  live: boolean;
  read_only: boolean;
  write: boolean;
  requires_model?: boolean;
  effort_supported?: boolean;
  reason: string | null;
}

export interface WorkflowScript {
  id: string;
  workspace_id: string;
  title: string;
  source: string;
  updated_at_ms: number;
}

export const DEFAULT_WORKFLOW_LIMITS: WorkflowLimits = {
  concurrency: 0,
  max_tasks: 128,
  max_attempts: 256,
  max_depth: 8,
  token_budget: null,
  max_output_bytes: 262144,
  wall_time_ms: 30 * 60 * 1000,
};

export const workflowCapabilities = () => invoke<WorkflowCapability[]>("workflow_capabilities");
export const workflowList = (workspaceId: string) => invoke<WorkflowRunSummary[]>("workflow_list", { workspaceId });
export const workflowGet = (runId: string) => invoke<WorkflowRunSnapshot>("workflow_get", { runId });
export const workflowCreate = (spec: WorkflowRunSpec, idempotencyKey: string) => invoke<WorkflowRunSnapshot>("workflow_create", { spec, idempotencyKey });
export const workflowPause = (runId: string) => invoke<WorkflowRunSnapshot>("workflow_pause", { runId });
export const workflowResume = (runId: string) => invoke<WorkflowRunSnapshot>("workflow_resume", { runId });
export const workflowCancelTask = (runId: string, taskId: string) => invoke<WorkflowRunSnapshot>("workflow_cancel_task", { runId, taskId });
export interface WorkflowIntegrationReport { attempt_id: string; digest: string; changed_paths: string[] }
export interface WorkflowArtifactFilePreview {
  path: string;
  before: string | null;
  after: string | null;
  binary: boolean;
  truncated: boolean;
  before_sha256: string | null;
  after_sha256: string | null;
}

export const workflowIntegrateArtifact = (runId: string, taskId: string, attemptId: string, digest: string) => invoke<WorkflowIntegrationReport>("workflow_integrate_artifact", { runId, taskId, attemptId, digest });
export const workflowArtifactPreview = (runId: string, taskId: string, attemptId: string, digest: string, path: string) => invoke<WorkflowArtifactFilePreview>("workflow_artifact_preview", { runId, taskId, attemptId, digest, path });
export const workflowCancel = (runId: string) => invoke<WorkflowRunSnapshot>("workflow_cancel", { runId });
export const workflowReconcile = (runId: string, taskId: string) => invoke<WorkflowRunSnapshot>("workflow_reconcile", { runId, taskId });
export const workflowRetry = (runId: string, taskId: string) => invoke<WorkflowRunSnapshot>("workflow_retry", { runId, taskId });
export const workflowReplace = (runId: string, taskId: string, spec: WorkflowTaskSpec, idempotencyKey = randomUUID()) => invoke<WorkflowRunSnapshot>("workflow_replace", { runId, taskId, spec, idempotencyKey });
export const workflowRetire = (runId: string, taskId: string) => invoke<WorkflowRunSnapshot>("workflow_retire", { runId, taskId });
export const workflowMessage = (runId: string, taskId: string, text: string) => invoke<WorkflowRunSnapshot>("workflow_message", { runId, taskId, text });
export const workflowScriptsList = (workspaceId: string) => invoke<WorkflowScript[]>("workflow_script_list", { workspaceId });
export const workflowScriptSave = (workspaceId: string, name: string, source: string) => invoke<WorkflowScript>("workflow_script_save", { workspaceId, name, source });
export const workflowScriptRun = (spec: WorkflowRunSpec, source: string, args: unknown, idempotencyKey: string) => invoke<WorkflowRunSnapshot>("workflow_script_execute", { input: { spec, source, args, idempotency_key: idempotencyKey } });

export function workflowError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") return error.message;
  return "The workflow could not be updated. Try again.";
}

export interface WorkflowChangeEvent { run_id: string | null; revision: number | null; kind?: string; timestamp_ms?: number; sequence?: number }
