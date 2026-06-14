import { describe, it, expect } from "vitest";
import {
  assembleAgentCommand,
  defaultAgentConfig,
  findAgentEntry,
} from "./agent-command";
import type { AgentCatalogEntry, PresetAgentConfig } from "@/tauri/types";

const claude: AgentCatalogEntry = {
  id: "claude",
  label: "Claude Code",
  icon: "claude",
  binary: "claude",
  autonomy_flag: "--dangerously-skip-permissions",
  accepts_model: true,
  model_flag: "--model",
  models: [
    { value: "opus", label: "Opus" },
    { value: "sonnet", label: "Sonnet" },
  ],
  reasoning: {
    flag_template: null,
    options: [
      { value: "", label: "Default", prompt_prefix: null },
      { value: "ultrathink", label: "Ultrathink", prompt_prefix: "Ultrathink. " },
    ],
  },
  supports_prompt: true,
};

const codex: AgentCatalogEntry = {
  id: "codex",
  label: "Codex",
  icon: "codex",
  binary: "codex",
  autonomy_flag: "--full-auto",
  accepts_model: true,
  model_flag: "--model",
  models: [],
  reasoning: {
    flag_template: '-c model_reasoning_effort="{value}"',
    options: [
      { value: "", label: "Default", prompt_prefix: null },
      { value: "high", label: "High", prompt_prefix: null },
    ],
  },
  supports_prompt: true,
};

const copilot: AgentCatalogEntry = {
  id: "copilot",
  label: "Copilot",
  icon: "copilot",
  binary: "copilot",
  autonomy_flag: "--allow-all",
  accepts_model: true,
  model_flag: "--model",
  models: [],
  reasoning: null,
  supports_prompt: true,
};

const cfg = (over: Partial<PresetAgentConfig>): PresetAgentConfig => ({
  agent_id: "claude",
  model: null,
  reasoning: null,
  prompt: "",
  skip_permissions: false,
  ...over,
});

describe("assembleAgentCommand", () => {
  it("emits just the binary when nothing else is set", () => {
    expect(assembleAgentCommand(claude, cfg({ skip_permissions: false }))).toBe(
      "claude",
    );
  });

  it("appends the autonomy flag when skip_permissions is on", () => {
    expect(assembleAgentCommand(claude, cfg({ skip_permissions: true }))).toBe(
      "claude --dangerously-skip-permissions",
    );
  });

  it("adds the model flag with the selected model", () => {
    expect(
      assembleAgentCommand(
        claude,
        cfg({ skip_permissions: true, model: "opus" }),
      ),
    ).toBe("claude --dangerously-skip-permissions --model opus");
  });

  it("omits the model flag when model is blank", () => {
    expect(
      assembleAgentCommand(claude, cfg({ model: "   " })),
    ).toBe("claude");
  });

  it("quotes the positional prompt", () => {
    expect(
      assembleAgentCommand(
        claude,
        cfg({ skip_permissions: true, model: "opus", prompt: "pull latest" }),
      ),
    ).toBe('claude --dangerously-skip-permissions --model opus "pull latest"');
  });

  it("prepends a prompt-prefix reasoning option to the prompt", () => {
    expect(
      assembleAgentCommand(
        claude,
        cfg({ reasoning: "ultrathink", prompt: "fix the bug" }),
      ),
    ).toBe('claude "Ultrathink. fix the bug"');
  });

  it("uses a real reasoning flag when the agent has one (codex)", () => {
    expect(
      assembleAgentCommand(
        codex,
        cfg({
          agent_id: "codex",
          skip_permissions: true,
          reasoning: "high",
          prompt: "run tests",
        }),
      ),
    ).toBe('codex --full-auto -c model_reasoning_effort="high" "run tests"');
  });

  it("ignores the Default (empty) reasoning option", () => {
    expect(
      assembleAgentCommand(codex, cfg({ agent_id: "codex", reasoning: "" })),
    ).toBe("codex");
  });

  it("escapes embedded double quotes in the prompt", () => {
    expect(
      assembleAgentCommand(claude, cfg({ prompt: 'say "hi"' })),
    ).toBe('claude "say \\"hi\\""');
  });

  it("leaves apostrophes untouched", () => {
    expect(
      assembleAgentCommand(claude, cfg({ prompt: "don't break" })),
    ).toBe('claude "don\'t break"');
  });

  it("works for agents with no reasoning block (copilot)", () => {
    expect(
      assembleAgentCommand(
        copilot,
        cfg({
          agent_id: "copilot",
          skip_permissions: true,
          model: "gpt-5",
          prompt: "push",
        }),
      ),
    ).toBe('copilot --allow-all --model gpt-5 "push"');
  });
});

describe("defaultAgentConfig", () => {
  it("defaults autonomy on for agents with an autonomy flag", () => {
    expect(defaultAgentConfig(claude).skip_permissions).toBe(true);
  });

  it("defaults autonomy off for agents without one", () => {
    const opencode: AgentCatalogEntry = { ...claude, id: "opencode", autonomy_flag: null };
    expect(defaultAgentConfig(opencode).skip_permissions).toBe(false);
  });
});

describe("findAgentEntry", () => {
  it("finds by id and returns null for misses", () => {
    expect(findAgentEntry([claude, codex], "codex")).toBe(codex);
    expect(findAgentEntry([claude, codex], "nope")).toBeNull();
    expect(findAgentEntry([claude], null)).toBeNull();
  });
});
