import { describe, expect, it } from "vitest";
import type { WorkflowCapability } from "@/tauri/workflows";
import {
  buildWorkflowRoutes,
  parseWorkflowCommand,
  parseWorkflowSettings,
  workflowLimits,
  workflowRouteId,
  workflowTitleFromGoal,
  type WorkflowModelOption,
  type WorkflowProviderKind,
} from "./workflow-settings";

const live = (provider: WorkflowCapability["provider"], extra: Partial<WorkflowCapability> = {}): WorkflowCapability => ({ provider, live: true, read_only: true, write: true, reason: null, ...extra });
const CATALOG: Record<string, WorkflowModelOption[]> = {
  claude: [{ id: "default", label: "Default" }, { id: "opus", label: "Opus 5.5" }, { id: "sonnet", label: "Sonnet 5" }, { id: "haiku", label: "Haiku 5" }],
  codex: [{ id: "gpt-5.5", label: "GPT-5.5" }, { id: "gpt-5.5-mini", label: "GPT-5.5 Mini" }],
  opencode: [{ id: "anthropic/claude-sonnet", label: "Claude Sonnet" }, { id: "bad model", label: "Bad" }],
};
const models = (provider: WorkflowProviderKind) => CATALOG[provider] ?? [];

describe("workflow settings", () => {
  it("defaults to same-provider, read-only, standard runs with Claude and Codex allowed and no composer button", () => {
    const settings = parseWorkflowSettings({});
    expect(settings).toMatchObject({ agents: "same_provider", allowWrites: false, size: "standard", tokenBudget: null, composerButton: false });
    expect(settings.providers.claude.enabled).toBe(true);
    expect(settings.providers.codex.enabled).toBe(true);
    expect(settings.providers.cursor.enabled).toBe(false);
  });

  it("reads stored choices and ignores corrupt provider JSON", () => {
    const settings = parseWorkflowSettings({
      "workflows.agents": "mixed",
      "workflows.file_changes": "allow",
      "workflows.size": "large",
      "workflows.token_budget": "120000",
      "workflows.composer_button": "true",
      "workflows.providers": "{not json",
    });
    expect(settings).toMatchObject({ agents: "mixed", allowWrites: true, size: "large", tokenBudget: 120000, composerButton: true });
    expect(settings.providers.claude.enabled).toBe(true);
    expect(parseWorkflowSettings({ "workflows.providers": JSON.stringify({ claude: { enabled: false, model: "sonnet" } }) }).providers.claude).toEqual({ enabled: false, model: "sonnet" });
  });

  it("turns the run size and token budget into engine limits", () => {
    const limits = workflowLimits(parseWorkflowSettings({ "workflows.size": "small", "workflows.token_budget": "5000" }));
    expect(limits).toMatchObject({ concurrency: 2, wall_time_ms: 15 * 60_000, token_budget: 5000 });
  });
});

describe("buildWorkflowRoutes", () => {
  it("keeps a same-provider run on the chat's model plus two lighter models of that provider", () => {
    const plan = buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [live("claude"), live("codex")], chat: { provider: "claude", model: "opus" }, models });
    expect(plan.problem).toBeNull();
    expect(plan.routes.map((route) => [route.provider, route.model])).toEqual([["claude", "opus"], ["claude", "sonnet"], ["claude", "haiku"]]);
    expect(plan.routes[0].id).toBe("claude-opus");
    expect(plan.labels["claude-sonnet"]).toBe("Claude Sonnet 5");
  });

  it("uses the provider default when the chat is on the default model alias", () => {
    const plan = buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [live("claude")], chat: { provider: "claude", model: "default" }, models });
    expect(plan.routes[0]).toMatchObject({ id: "claude", provider: "claude", model: null });
  });

  it("explains why a chat provider cannot run workflows", () => {
    const plan = buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [live("grok", { live: false, reason: "No fan-out boundary" })], chat: { provider: "grok", model: null }, models });
    expect(plan.routes).toEqual([]);
    expect(plan.problem).toBe("Grok can't run workflows here: No fan-out boundary");
    expect(buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [], chat: { provider: "acp", model: null }, models }).problem).toMatch(/can't run on this chat's agent/);
  });

  it("requires a valid provider/model for OpenCode and skips invalid catalog entries", () => {
    const capabilities = [live("opencode", { requires_model: true })];
    expect(buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities, chat: { provider: "opencode", model: null }, models }).problem).toBe("Choose a model for OpenCode in the chat before starting a workflow.");
    const plan = buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities, chat: { provider: "opencode", model: "openai/gpt-5.5" }, models });
    expect(plan.routes.map((route) => route.model)).toEqual(["openai/gpt-5.5", "anthropic/claude-sonnet"]);
  });

  it("adds every enabled, ready provider when mixing, after the chat's own model", () => {
    const settings = parseWorkflowSettings({
      "workflows.agents": "mixed",
      "workflows.providers": JSON.stringify({ cursor: { enabled: true, model: "" }, opencode: { enabled: true, model: "" } }),
    });
    const plan = buildWorkflowRoutes({
      settings,
      capabilities: [live("claude"), live("codex"), live("cursor"), live("opencode", { requires_model: true })],
      chat: { provider: "claude", model: "opus" },
      models,
    });
    // OpenCode is enabled but has no model, so it is left out.
    expect(plan.routes.map((route) => route.id)).toEqual(["claude-opus", "codex", "cursor"]);
  });

  it("uses enabled providers for runs started outside a chat", () => {
    const plan = buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [live("claude"), live("codex", { live: false })], chat: null, models });
    expect(plan.routes.map((route) => route.id)).toEqual(["claude"]);
    expect(buildWorkflowRoutes({ settings: parseWorkflowSettings({}), capabilities: [], chat: null, models }).problem).toMatch(/No workflow provider is ready/);
  });
});

describe("workflow command helpers", () => {
  it("parses /workflow with or without a goal and ignores other text", () => {
    expect(parseWorkflowCommand("/workflow  review the auth module ")).toEqual({ goal: "review the auth module" });
    expect(parseWorkflowCommand("/workflow\nline one\nline two")).toEqual({ goal: "line one\nline two" });
    expect(parseWorkflowCommand("/workflow")).toEqual({ goal: "" });
    expect(parseWorkflowCommand("/workflows please")).toBeNull();
    expect(parseWorkflowCommand("run /workflow later")).toBeNull();
  });

  it("titles a run from the first line of its goal", () => {
    expect(workflowTitleFromGoal("Review auth\nthen fix it")).toBe("Review auth");
    expect(workflowTitleFromGoal("x".repeat(100))).toHaveLength(72);
    expect(workflowTitleFromGoal("   ")).toBe("Workflow");
  });

  it("builds engine-safe route ids", () => {
    expect(workflowRouteId("claude", "claude-opus-5-5[1m]")).toBe("claude-claude-opus-5-5-1m");
    expect(workflowRouteId("codex", null)).toBe("codex");
    expect(workflowRouteId("opencode", "anthropic/claude sonnet")).toMatch(/^[A-Za-z0-9_.:-]+$/);
  });
});
