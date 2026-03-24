import type { CommLogEntry, OpenFlowRunRecord } from "@/tauri/types";

// Polling intervals (ms)
export const POLL_ACTIVE = 3000;
export const POLL_COMPLETED = 10000;
export const POLL_RUNTIME_FALLBACK = 10000;
export const MAX_VISIBLE_MESSAGES = 100;

const SYSTEM_MARKERS = [
  "HANDLED_INJECTIONS:",
  "HANDLED_ASSIGNMENTS:",
  "DONE_RELAY_COUNT:",
  "INJECTION_PENDING:",
];

export function filterSystemMarkers(entries: CommLogEntry[]): CommLogEntry[] {
  return entries.filter(
    (e) =>
      e.role !== "system" ||
      !SYSTEM_MARKERS.some((marker) => e.message.trimStart().startsWith(marker)),
  );
}

export function commLogPollInterval(run: OpenFlowRunRecord | null): number {
  if (!run) return POLL_ACTIVE;
  return isTerminalStatus(run.status) ? POLL_COMPLETED : POLL_ACTIVE;
}

export function isTerminalStatus(
  status: OpenFlowRunRecord["status"],
): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}

const ROLE_COLORS: Record<string, string> = {
  orchestrator: "text-emerald-400",
  builder: "text-amber-400",
  tester: "text-cyan-400",
  reviewer: "text-violet-400",
  planner: "text-blue-400",
  researcher: "text-sky-400",
  debugger: "text-orange-400",
  system: "text-muted-foreground",
  "user/inject": "text-primary",
};

export function getRoleColor(role: string): string {
  const lower = role.toLowerCase();
  // Check exact match first, then prefix match (for "builder-0" etc.)
  if (ROLE_COLORS[lower]) return ROLE_COLORS[lower];
  const prefix = lower.replace(/-\d+$/, "");
  return ROLE_COLORS[prefix] ?? "text-muted-foreground";
}

const ROLE_BG_COLORS: Record<string, string> = {
  orchestrator: "bg-emerald-400/10",
  builder: "bg-amber-400/10",
  tester: "bg-cyan-400/10",
  reviewer: "bg-violet-400/10",
  planner: "bg-blue-400/10",
  researcher: "bg-sky-400/10",
  debugger: "bg-orange-400/10",
  system: "bg-muted/50",
  "user/inject": "bg-primary/10",
};

export function getRoleBgColor(role: string): string {
  const lower = role.toLowerCase();
  if (ROLE_BG_COLORS[lower]) return ROLE_BG_COLORS[lower];
  const prefix = lower.replace(/-\d+$/, "");
  return ROLE_BG_COLORS[prefix] ?? "bg-muted/50";
}

const PHASE_LABELS: Record<string, string> = {
  plan: "Planning",
  planning: "Planning",
  assign: "Assigning",
  assigning: "Assigning",
  execute: "Executing",
  executing: "Executing",
  verify: "Verifying",
  verifying: "Verifying",
  review: "Reviewing",
  reviewing: "Reviewing",
  awaiting_approval: "Awaiting Approval",
  waiting_approval: "Awaiting Approval",
  complete: "Completed",
  completed: "Completed",
  replan: "Replanning",
  replanning: "Replanning",
  blocked: "Blocked",
};

export function phaseLabel(phase: string): string {
  return PHASE_LABELS[phase.toLowerCase()] ?? phase;
}

type StatusTone = "success" | "danger" | "warning" | "muted";

const STATUS_TONES: Record<string, StatusTone> = {
  completed: "success",
  done: "success",
  passed: "success",
  failed: "danger",
  cancelled: "danger",
  blocked: "danger",
  dead: "danger",
  active: "warning",
  executing: "warning",
  planning: "warning",
  verifying: "warning",
  reviewing: "warning",
  awaiting_approval: "warning",
  idle: "muted",
  draft: "muted",
  pending: "muted",
};

export function getStatusTone(status: string): StatusTone {
  return STATUS_TONES[status.toLowerCase()] ?? "muted";
}

const TONE_DOT_CLASSES: Record<StatusTone, string> = {
  success: "bg-emerald-400",
  danger: "bg-red-400",
  warning: "bg-amber-400",
  muted: "bg-muted-foreground/50",
};

export function getStatusDotClass(status: string): string {
  return TONE_DOT_CLASSES[getStatusTone(status)];
}

const TONE_BADGE_CLASSES: Record<StatusTone, string> = {
  success: "border-emerald-400/30 text-emerald-400 bg-emerald-400/10",
  danger: "border-red-400/30 text-red-400 bg-red-400/10",
  warning: "border-amber-400/30 text-amber-400 bg-amber-400/10",
  muted: "border-border text-muted-foreground bg-muted/50",
};

export function getPhaseBadgeClass(phase: string): string {
  return TONE_BADGE_CLASSES[getStatusTone(phase)];
}

const HEALTH_LABELS: Record<string, { label: string; tone: StatusTone }> = {
  active: { label: "Active", tone: "success" },
  idle: { label: "Idle", tone: "muted" },
  initializing: { label: "Initializing", tone: "muted" },
  waiting_for_response: { label: "Waiting", tone: "warning" },
  correcting_delegation: { label: "Correcting", tone: "warning" },
  stalled: { label: "Stalled", tone: "danger" },
  error: { label: "Error", tone: "danger" },
};

export function getHealthInfo(
  state: OpenFlowRunRecord["orchestration_state"],
): { label: string; tone: StatusTone } {
  return HEALTH_LABELS[state] ?? { label: state, tone: "muted" };
}

export function formatRole(role: string): string {
  return role
    .replace(/-(\d+)$/, " $1")
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

const ROLE_EMOJIS: Record<string, string> = {
  orchestrator: "🎯",
  planner: "📋",
  builder: "🔨",
  reviewer: "🔍",
  tester: "🧪",
  debugger: "🐛",
  researcher: "📚",
};

export function getRoleEmoji(role: string): string {
  const prefix = role.toLowerCase().replace(/-\d+$/, "");
  return ROLE_EMOJIS[prefix] ?? "⚙️";
}
