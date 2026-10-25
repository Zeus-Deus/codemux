/** Synthetic platform plugins for UI verification; this module only loads in the dev mock. */
import type { PluginInstallation, PluginSource } from "@/components/plugins/contract";

type Args = Record<string, unknown>;
const demo: PluginInstallation = {
  installationId: "mock-workflow-board", generation: "mock-generation-1", dataOwner: "mock-data-owner",
  manifest: {
    id: "examples.workflow-board", name: "Workflow Board", description: "Review task plans and follow completed turns.",
    version: "1.0.0", apiVersion: 1, entry: "plugin.mjs",
    capabilities: ["storage", "settings", "threads.read", "threads.create", "workflow.query", "ui", "ui.conversation-cards", "ui.native-trees", "ui.sidebar-views", "ui.changes", "events.run-finalized"],
    actions: [{ id: "open", title: "Open workflow board", placements: ["palette", "thread-menu"], agentCallable: true }, { id: "save", title: "Save selection to board", placements: ["message-selection"] }],
    views: [{ id: "board", title: "Workflow Board", entry: "view.html", placement: "page" }, { id: "inspector", title: "Task inspector", entry: "view.html", placement: "side-panel" }, { id: "tasks", title: "Task proposal", entry: "view.html", placement: "conversation" }],
    nativeTrees: [{ id: "projects", title: "Projects and tasks" }],
    sidebarViews: [{ id: "by-project", title: "All project chats", groupBy: "project", emptyProjects: "show" }],
    threadStatus: [{ id: "tasks", title: "Tasks" }],
    settings: [{ key: "followUps", label: "Automatic follow-ups", type: "boolean", default: false }, { key: "token", label: "Service token", type: "secret" }],
  },
  source: { kind: "local", directory: "/demo/plugins/workflow-board" },
  digest: "a".repeat(64), consentedDigest: "a".repeat(64), enabled: true, status: "idle", error: null,
  compatibility: { available: true, missingCapabilities: [] },
  grants: { capabilities: ["storage", "settings", "threads.read", "threads.create", "workflow.query", "ui", "ui.conversation-cards", "ui.native-trees", "ui.sidebar-views", "ui.changes", "events.run-finalized"], projectIds: [] },
  settings: { followUps: false }, updatedAt: "2026-10-24T12:00:00Z",
};

/** A realistic collection with duplicate names and states that need intervention. */
function pluginCollection(): PluginInstallation[] {
  const make = (
    id: string,
    name: string,
    description: string,
    state: Partial<PluginInstallation> = {},
    manifest: Partial<PluginInstallation["manifest"]> = {},
  ): PluginInstallation => ({
    ...structuredClone(demo),
    installationId: id,
    generation: `${id}-generation`,
    dataOwner: `${id}-data`,
    source: { kind: "git", url: `https://plugins.example.com/team/${id}.git`, ref: "main" },
    ...state,
    manifest: {
      ...structuredClone(demo.manifest), id: `examples.${id}`, name, description,
      actions: [{ id: "open", title: `Open ${name}`, placements: ["palette"], agentCallable: true }],
      views: [{ id: "main", title: name, entry: "view.html", placement: "page" }],
      sidebarViews: [], nativeTrees: [], threadStatus: [],
      ...manifest,
    },
  });
  const board = structuredClone(demo);
  board.manifest.settings = [
    { key: "title", label: "Board title", type: "string", default: "My workflow" },
    { key: "taskLimit", label: "Tasks per project", type: "number", default: 8 },
    ...board.manifest.settings,
  ];
  board.settings = { ...board.settings, title: "My workflow", taskLimit: 8 };
  board.storedSecrets = ["token"];
  board.grants = { ...board.grants, projectIds: ["uid-codemux"] };
  const launcher = make("mock-task-launcher", "Task Launcher", "Review task proposals before starting chats in a project or worktree.", {}, { version: "1.3.0" });
  launcher.pendingUpdate = {
    kind: "upstream", manifest: { ...launcher.manifest, version: "1.4.0" }, source: launcher.source,
    digest: "b".repeat(64), generation: "launcher-pending", updatedAt: demo.updatedAt,
  };
  const lens = make("mock-repository-lens", "Repository Lens", "Explore project activity and keep a view of work in progress.", {
    status: "quarantined", error: "The last three background invocations exceeded their deadline.",
    attention: { kind: "fault", message: "Stopped after repeated timeouts. Your data is kept.", since: demo.updatedAt },
    health: {
      lastHealthy: { hostVersion: "0.26.1", nodeVersion: "24.0.0", at: "2026-10-23T12:00:00Z" },
      fault: { hostVersion: "0.26.2", nodeVersion: "24.0.0", at: demo.updatedAt, reason: "timeout", message: "Background handler exceeded its deadline.", handler: "event:run.finalized" },
    },
    recovery: {
      activeSource: "upstream",
      previous: { version: "1.1.0", digest: "c".repeat(64), source: { kind: "git", url: "https://plugins.example.com/team/repository-lens.git" } },
      repair: { workspaceId: "mock-lens-repair", directory: "/demo/repairs/repository-lens", baseDigest: demo.digest, baseGeneration: "mock-repository-lens-generation", draft: "Repair Repository Lens and preserve stored data." },
    },
  }, { version: "1.2.0" });
  const secondBoard = make("mock-workflow-board-local", "Workflow Board", "A local development copy with separate data and project access.", {
    enabled: false, status: "disabled", source: { kind: "local", directory: "/demo/experiments/workflow-board" },
  }, { id: demo.manifest.id, version: "1.1.0-dev" });
  return [
    board,
    make("mock-projects-tree", "Projects Tree", "Group chats by project while keeping Codemux's native sidebar rows.", {}, {
      views: [], sidebarViews: [{ id: "by-project", title: "All project chats", groupBy: "project", emptyProjects: "show" }],
      nativeTrees: [{ id: "projects", title: "Projects and tasks" }],
    }),
    launcher,
    lens,
    make("mock-quote-helper", "Quote Helper", "Collect selected replies and add them to a draft.", {
      enabled: false, status: "disabled",
      recovery: { activeSource: "local-repair", upstreamSource: { kind: "git", url: "https://plugins.example.com/community/quote-helper.git" }, previous: { version: "0.9.0", digest: "c".repeat(64), source: { kind: "git", url: "https://plugins.example.com/community/quote-helper.git" } } },
    }, { actions: [{ id: "quote", title: "Quote selection", placements: ["message-selection"] }], views: [] }),
    make("mock-notes-sync", "Notes Sync", "Save completed work to a private notebook.", { enabled: false, status: "needs-consent", consentedDigest: null, grants: { capabilities: [], projectIds: [] } }),
    make("mock-preview-tools", "Preview Tools", "Tools for the next version of the plugin API.", {
      status: "incompatible", compatibility: { available: false, missingCapabilities: ["ui.future-preview"] },
      error: "This version requires a feature this Codemux build does not support.",
    }, { apiVersion: 2, capabilities: ["ui", "ui.future-preview"] }),
    secondBoard,
    make(`plugin-${"d".repeat(64)}`, "Build Insights for Shared Development Environments", "Track long-running builds across project workspaces without changing the selected chat.", {
      observerError: "Choose how to handle two runs missed while this plugin was off.",
      attention: { kind: "events-paused", message: "Two missed runs need a deliver or skip choice.", since: demo.updatedAt },
      storedSecrets: ["token"],
    }),
    make("mock-retained-record", "Retained plugin record", "Saved by a newer host. Its files and data are kept.", {
      status: "incompatible", recordIssue: "newer", enabled: false,
      attention: { kind: "read-only", message: "This record needs a newer Codemux version.", since: demo.updatedAt },
    }),
    make("mock-commit-notes", "Commit Notes", "Keep a small journal of the changes you review.", { consentedDigest: "f".repeat(64), status: "needs-consent" }),
    make("mock-workspace-notes", "Workspace Notes", "Pin project notes beside your agent's chat.", {
      pendingUpdate: {
        kind: "local-repair", manifest: { ...demo.manifest, id: "examples.workspace-notes", name: "Workspace Notes", version: "1.0.1" },
        source: { kind: "local", directory: "/demo/repairs/workspace-notes" },
        digest: "e".repeat(64), generation: "notes-local-fix", updatedAt: demo.updatedAt,
      },
    }),
    make("mock-off-unavailable", "Preview Tools Classic", "An older preview integration kept for reference.", {
      enabled: false, status: "incompatible", compatibility: { available: false, missingCapabilities: ["ui.retired-preview"] },
    }),
    make("mock-unreadable-record", "Unreadable plugin record", "Its saved files are retained for inspection.", {
      enabled: false, status: "incompatible", recordIssue: "unreadable",
    }, { version: "0.0.0" }),
  ];
}

// Match Rust's nullable optional fields so visual checks exercise the wire decoder.
const wireInstallation = (installation: PluginInstallation) => ({
  ...structuredClone(installation),
  pendingUpdate: installation.pendingUpdate ?? null,
  health: installation.health ?? null,
  attention: installation.attention ?? null,
  recordIssue: installation.recordIssue ?? null,
  observerError: installation.observerError ?? null,
  recovery: installation.recovery ?? { activeSource: "upstream", previous: null, repair: null, upstreamSource: installation.source },
  settings: { ...installation.settings, ...Object.fromEntries((installation.storedSecrets ?? []).map((key) => [key, { hasSecret: true }])) },
});

const view = `<!doctype html><html><head><meta charset="utf-8"><style>body{font:14px system-ui;padding:20px;color:var(--foreground);background:var(--background)}h1{font-size:22px}p{line-height:1.6;color:var(--muted-foreground)}button{font:inherit;padding:9px 14px;border:1px solid var(--border);border-radius:8px;background:var(--primary);color:var(--primary-foreground)}</style></head><body><h1>Workflow Board</h1><p>Review the proposal before starting independent tasks. This page uses the isolated plugin bridge.</p><button id="refresh">Refresh board</button> <button id="preview">Preview guide</button><p id="status">Ready for review</p><script>const bridge=window.codemux;bridge.ready.then(()=>{document.getElementById('status').textContent='Connected to Codemux';});document.getElementById('preview').onclick=()=>bridge.invoke('preview',{});document.getElementById('refresh').onclick=async()=>{try{const result=await bridge.invoke('read',{});document.getElementById('status').textContent=result.message;}catch{document.getElementById('status').textContent='Plugin unavailable';}};</script></body></html>`;

export function platformPluginMockHandlers(): Record<string, (args: Args) => unknown> {
  const fixture = new URLSearchParams(location.search).get("platformPlugins");
  let installations: PluginInstallation[] = fixture === "collection" ? pluginCollection() : fixture === "demo" ? [structuredClone(demo)] : [];
  const resolvedBacklogs = new Set<string>();
  const find = (id: unknown) => {
    const installation = installations.find((p) => p.installationId === id);
    if (!installation) throw new Error("not_found: Plugin is unavailable");
    return installation;
  };
  return { plugins_platform_request: (args) => {
    const method = String(args.method);
    const params = (args.params && typeof args.params === "object" ? args.params : {}) as Record<string, unknown>;
    if (method === "catalogue") return { installations: installations.map(wireInstallation), recoverySupported: true, changesSupported: true, conversationCardsSupported: true, executionDisabled: false };
    if (method === "install") {
      const installation = structuredClone(demo);
      installation.installationId = `mock-plugin-${installations.length + 1}`;
      installation.source = params.source as PluginSource;
      installation.enabled = false; installation.consentedDigest = null; installation.status = "needs-consent";
      installations = [...installations, installation]; return wireInstallation(installation);
    }
    const installation = find(params.installationId);
    if (method === "readView" || method === "readCardView") return { html: view.split("Workflow Board").join(installation.manifest.name) };
    if (method === "readTree") return { nodes: [{ id: "work", kind: "group", title: "Plugin projects" }] };
    if (method === "invoke") {
      if (!installation.enabled || installation.status === "incompatible") throw new Error("disabled: Plugin is unavailable");
      if (params.generation !== installation.generation) throw new Error("stale_generation: Refresh the plugin");
      const handler = String(params.handler);
      if (handler.startsWith("status:")) return { data: { label: "2 tasks ready", tone: "info" } };
      if (handler.endsWith(":preview")) return { intents: [{ type: "preview.open", url: "https://example.com/" }] };
      if (handler.startsWith("view:")) return { message: "Board refreshed — 2 tasks ready for review" };
      const page = installation.manifest.views.find((entry) => entry.placement === "page");
      return { message: `${installation.manifest.name} opened`, ...(page ? { intents: [{ type: "view.open", viewId: page.id }] } : {}) };
    }
    if (method === "backlog") return { count: resolvedBacklogs.has(installation.installationId) ? 0 : 2, afterSequence: 10, throughSequence: 12, requiresChoice: !resolvedBacklogs.has(installation.installationId) };
    if (params.eventBacklog) resolvedBacklogs.add(installation.installationId);
    if (method === "refresh") {
      installation.pendingUpdate = { manifest: { ...installation.manifest, version: "1.1.0" }, source: installation.source, digest: "b".repeat(64), generation: "mock-pending", updatedAt: "2026-10-24T12:10:00Z", kind: "upstream" };
    } else if (method === "discardPendingUpdate") delete installation.pendingUpdate;
    else if (method === "consent") {
      if (installation.pendingUpdate) {
        const pending = installation.pendingUpdate;
        installation.recovery = {
          ...installation.recovery,
          activeSource: pending.kind === "local-repair" ? "local-repair" : "upstream",
          previous: { version: installation.manifest.version, digest: installation.digest, source: installation.source },
        };
        installation.manifest = pending.manifest; installation.source = pending.source; installation.digest = pending.digest; delete installation.pendingUpdate;
      }
      installation.consentedDigest = installation.digest; installation.status = installation.enabled ? "idle" : "disabled";
      installation.generation = `mock-generation-${Date.now()}`;
      if (params.grants) installation.grants = params.grants as PluginInstallation["grants"];
    } else if (method === "setEnabled") { installation.enabled = params.enabled === true; installation.status = installation.enabled ? "idle" : "disabled"; }
    else if (method === "resume") { installation.status = installation.enabled ? "idle" : "disabled"; installation.error = null; delete installation.attention; delete installation.observerError; }
    else if (method === "updateSettings") {
      const settings = { ...installation.settings };
      const secrets = new Set(installation.storedSecrets ?? []);
      for (const [key, value] of Object.entries((params.values ?? {}) as Record<string, unknown>)) {
        const setting = installation.manifest.settings.find((entry) => entry.key === key);
        if (setting?.type === "secret") {
          if (value === "") secrets.delete(key); else secrets.add(key);
        } else if (typeof value === "string" || typeof value === "boolean" || typeof value === "number") settings[key] = value;
      }
      installation.settings = settings; installation.storedSecrets = [...secrets];
    }
    else if (method === "stagePrevious") {
      const previous = installation.recovery?.previous;
      if (!previous || previous.digest !== params.expectedDigest) throw new Error("conflict: Previous version changed");
      installation.pendingUpdate = { kind: "previous", manifest: { ...installation.manifest, version: previous.version }, source: previous.source, digest: previous.digest, generation: "mock-previous", updatedAt: demo.updatedAt };
    }
    else if (method === "stageRepair") {
      const repair = installation.recovery?.repair;
      if (!repair || repair.workspaceId !== params.workspaceId || repair.baseDigest !== params.baseDigest) throw new Error("conflict: Repair folder changed");
      installation.pendingUpdate = { kind: "local-repair", manifest: installation.manifest, source: { kind: "local", directory: repair.directory }, digest: "e".repeat(64), generation: "mock-repaired", updatedAt: demo.updatedAt };
    }
    else if (method === "remove") { installations = installations.filter((p) => p !== installation); return null; }
    else if (method === "prepareRepair") {
      const repair = { workspaceId: `repair-${installation.installationId}`, directory: `/demo/repairs/${installation.installationId}`, baseDigest: installation.digest, baseGeneration: installation.generation, draft: "Repair this plugin using its public API and retain stored data." };
      installation.recovery = { ...installation.recovery, activeSource: installation.recovery?.activeSource ?? "upstream", repair };
      return repair;
    }
    else throw new Error("unsupported_operation: This mock does not implement that operation");
    return wireInstallation(installation);
  } };
}
