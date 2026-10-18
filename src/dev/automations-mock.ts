/**
 * Dev-mock handlers for every `automations_*` and `secrets_*` command.
 *
 * Stateful, so the Automations page can create, edit, pause, delete and
 * configure webhooks end to end in a browser. Secrets keep metadata only,
 * mirroring the backend, which never returns a stored value.
 */
import type {
  AutomationInput,
  AutomationRunView,
  AutomationView,
  SecretMeta,
  WebhookCredentials,
  WebhookSignature,
} from "@/tauri/commands";

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

const DAY_MS = 24 * 60 * 60 * 1000;
const SEEDED_WEBHOOK_ID = "c41d2a7f9e0b4b6a8d13f5e7a9c2b4d6";

function iso(offsetMs: number): string {
  return new Date(Date.now() + offsetMs).toISOString();
}

function seedAutomations(): AutomationView[] {
  const base = {
    server_id: null,
    agent: "claude",
    timezone: "UTC",
    host_id: null,
    project_path: "/home/dev/projects/acme-web",
    project_remote: "git@github.com:acme/acme-web.git",
    enabled: true,
    retention_limit: 10,
    created_at: iso(-30 * DAY_MS),
    updated_at: iso(-2 * DAY_MS),
    dirty: false,
    trigger_type: "schedule" as const,
    webhook: null,
  };
  return [
    {
      ...base,
      id: 1,
      name: "Daily issue triage",
      prompt: "Label new issues, close duplicates, and summarise anything urgent.",
      schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY;BYHOUR=9;BYMINUTE=0",
      last_run_at: iso(-3 * 60 * 60 * 1000),
      next_run_at: iso(21 * 60 * 60 * 1000),
      last_run_status: "succeeded",
    },
    {
      ...base,
      id: 2,
      name: "Weekly dependency bump",
      prompt: "Update dependencies, run the tests, and open a PR if they pass.",
      schedule: "DTSTART:20260105T070000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO;BYHOUR=7;BYMINUTE=0",
      last_run_at: iso(-5 * DAY_MS),
      next_run_at: iso(2 * DAY_MS),
      last_run_status: "failed",
    },
    {
      ...base,
      id: 3,
      name: "Triage new GitHub issues",
      prompt:
        "Triage issue #{{body.issue.number}}: {{body.issue.title}}\n\n{{body.issue.body}}\n\nLabel it and reply with next steps.",
      schedule: "",
      last_run_at: iso(-40 * 60 * 1000),
      next_run_at: null,
      last_run_status: "succeeded",
      trigger_type: "webhook" as const,
      webhook: { id: SEEDED_WEBHOOK_ID, signature: null, secret_ref: null },
    },
  ];
}


function seedRuns(automations: AutomationView[]): AutomationRunView[] {
  const runs: AutomationRunView[] = [];
  let id = 1;
  for (const automation of automations) {
    for (let i = 0; i < 3; i += 1) {
      const at = iso(-(i + 1) * DAY_MS);
      const failed = automation.last_run_status === "failed" && i === 0;
      runs.push({
        id: id++,
        automation_id: automation.id,
        status: failed ? "failed" : "succeeded",
        scheduled_for: at,
        started_at: at,
        finished_at: at,
        host_id: null,
        workspace_id: null,
        branch: `automation-${automation.id}-${i}`,
        pr_url: i === 1 ? "https://github.com/acme/acme-web/pull/128" : null,
        error: failed ? "Agent 'claude' exited with exit status: 1" : null,
        created_at: at,
        trigger_type: automation.trigger_type,
      });
    }
  }
  return runs;
}

let tokenCounter = 0;
function mockToken(): string {
  tokenCounter += 1;
  return `${"7f3a9c1e5b2d4f60".repeat(4).slice(0, 60)}${String(tokenCounter).padStart(4, "0")}`;
}

export function automationMockHandlers(): Record<string, Handler> {
  const automations = seedAutomations();
  const runs = seedRuns(automations);
  const tokens = new Map<number, WebhookCredentials>([
    [3, { webhook_id: SEEDED_WEBHOOK_ID, token: mockToken() }],
  ]);
  const secrets: SecretMeta[] = [];
  let nextId = automations.length + 1;

  const find = (id: unknown): AutomationView => {
    const automation = automations.find((a) => a.id === Number(id));
    if (!automation) throw new Error(`No automation with id ${String(id)}`);
    return automation;
  };
  const credentials = (automation: AutomationView): WebhookCredentials => {
    const found = tokens.get(automation.id);
    if (!automation.webhook || !found) {
      throw new Error(`Automation ${automation.id} is not triggered by a webhook`);
    }
    return { ...found };
  };

  return {
    automations_list: () => automations.map((a) => structuredClone(a)),
    automations_get: (a) => structuredClone(find(a.id)),
    automations_create: (a) => {
      const input = a.input as AutomationInput;
      const trigger = input.trigger_type ?? "schedule";
      const id = nextId++;
      const webhookId = `mockhook${String(id).padStart(24, "0")}`;
      const automation: AutomationView = {
        id,
        server_id: null,
        name: input.name,
        prompt: input.prompt,
        agent: input.agent,
        schedule: trigger === "webhook" ? "" : input.schedule,
        timezone: input.timezone,
        host_id: input.host_id,
        project_path: input.project_path,
        project_remote: input.project_remote ?? null,
        enabled: true,
        retention_limit: input.retention_limit,
        last_run_at: null,
        next_run_at: trigger === "webhook" ? null : iso(DAY_MS),
        last_run_status: null,
        created_at: iso(0),
        updated_at: iso(0),
        dirty: true,
        trigger_type: trigger,
        webhook:
          trigger === "webhook" ? { id: webhookId, signature: null, secret_ref: null } : null,
      };
      if (trigger === "webhook") tokens.set(id, { webhook_id: webhookId, token: mockToken() });
      automations.push(automation);
      return structuredClone(automation);
    },
    automations_update: (a) => {
      const automation = find(a.id);
      const input = a.input as AutomationInput;
      Object.assign(automation, {
        name: input.name,
        prompt: input.prompt,
        agent: input.agent,
        schedule: automation.trigger_type === "webhook" ? "" : input.schedule,
        timezone: input.timezone,
        host_id: input.host_id,
        project_path: input.project_path,
        retention_limit: input.retention_limit,
        updated_at: iso(0),
      });
      return structuredClone(automation);
    },
    automations_set_enabled: (a) => {
      const automation = find(a.id);
      automation.enabled = Boolean(a.enabled);
      automation.next_run_at =
        automation.enabled && automation.trigger_type === "schedule" ? iso(DAY_MS) : null;
      return structuredClone(automation);
    },
    automations_delete: (a) => {
      automations.splice(automations.indexOf(find(a.id)), 1);
      return undefined;
    },
    automations_runs: (a) =>
      runs
        .filter((run) => run.automation_id === Number(a.automationId))
        .slice(0, Number(a.limit ?? 20))
        .map((run) => ({ ...run })),
    automations_check_repo_access: () => ({
      ok: true,
      message: "This host can reach the repository.",
    }),
    automations_webhook_credentials: (a) => credentials(find(a.id)),
    automations_rotate_webhook_token: (a) => {
      const automation = find(a.id);
      const current = credentials(automation);
      const rotated = { webhook_id: current.webhook_id, token: mockToken() };
      tokens.set(automation.id, rotated);
      return { ...rotated };
    },
    automations_set_webhook_signature: (a) => {
      const automation = find(a.id);
      if (!automation.webhook) throw new Error(`No webhook automation with id ${automation.id}`);
      const input = a.signature as (WebhookSignature & { secret_ref: string }) | null;
      if (input && !secrets.some((s) => s.id === input.secret_ref)) {
        throw new Error("That secret no longer exists. Enter it again.");
      }
      automation.webhook = {
        id: automation.webhook.id,
        signature: input
          ? {
              header: input.header.trim().toLowerCase(),
              encoding: input.encoding,
              prefix: input.prefix,
            }
          : null,
        secret_ref: input ? input.secret_ref : null,
      };
      return structuredClone(automation);
    },
    secrets_store: (a) => {
      const meta: SecretMeta = {
        id: `secret_mock${secrets.length + 1}`,
        label: String(a.label ?? "").trim(),
        created_at: iso(0),
      };
      secrets.unshift(meta);
      return { ...meta };
    },
    secrets_list: () => secrets.map((secret) => ({ ...secret })),
    secrets_delete: (a) => {
      const user = automations.find((x) => x.webhook?.secret_ref === a.id);
      if (user) {
        throw new Error(
          `The automation "${user.name}" uses this secret. Remove its signature check first.`,
        );
      }
      const index = secrets.findIndex((secret) => secret.id === a.id);
      if (index === -1) throw new Error("No such secret");
      secrets.splice(index, 1);
      return undefined;
    },
  };
}
