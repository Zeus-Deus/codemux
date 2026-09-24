/**
 * Browser-preview add-on manager fixtures for `src/dev/tauri-mock.ts`.
 *
 * Off by default: without `?addons=…` the preview has no add-ons, exactly
 * like a clean install, so the core-only UI is what screenshots show. The
 * opt-in modes exist so the Settings → Add-ons states can be previewed and
 * captured with synthetic data only (invented names, example.com services):
 *
 *   ?addons=fixture        installed rows in every lifecycle state, a fresh
 *                          cached catalog, install/update reviews
 *   ?addons=offline        the same, with a stale catalog and a failed refresh
 *   ?addons=interrupted    paused after an unclean exit during activation
 *   ?addons=registry-error the registry could not be opened; Reset recovers
 *
 * Nothing here runs plugin code. Commands without a handler reject like the
 * real host does for an unavailable operation instead of resolving `null`.
 */
import { MOCK_HOME_DIR } from "./mock-fixtures";
import type {
  AddonAccess,
  AddonCatalogBrowse,
  AddonCatalogPlugin,
  AddonCredentialState,
  AddonDiagnostics,
  AddonInstallation,
  AddonListing,
  AddonManifest,
  AddonReview,
  AddonSource,
} from "@/lib/addons/types";

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

const MODES = ["fixture", "offline", "interrupted", "registry-error"] as const;
type Mode = (typeof MODES)[number] | null;
function readMode(): Mode {
  const value = new URLSearchParams(location.search).get("addons");
  return (MODES as readonly string[]).includes(value ?? "")
    ? (value as Mode)
    : null;
}

const fail = (code: string, message: string) => ({ message, data: { code } });
const PLATFORM = "linux-x64";
const HOST_API = "1.0.0";
const DATA_DIR = `${MOCK_HOME_DIR}/.local/share/codemux-dev/addons-v1`;
// Synthetic, well-formed digests: 64 lowercase hex characters each.
const digest = (seed: number) =>
  Array.from({ length: 64 }, (_, i) => ((seed * 7 + i * 13) % 16).toString(16)).join("");

function manifest(
  fields: Pick<AddonManifest, "id" | "name" | "version" | "description"> &
    Partial<AddonManifest>,
): AddonManifest {
  return {
    api: "^1.0.0",
    author: { name: "Example authors", url: "https://example.com" },
    repository: `https://github.com/example/${fields.id.split(".")[1]}`,
    license: "MIT",
    platforms: ["linux-x64", "windows-x64"],
    permissions: [],
    http: [],
    credentials: [],
    contributes: {
      commands: [],
      panels: [],
      composerActions: [],
      composerViews: [],
    },
    settings: [],
    ...fields,
  };
}

const standup = (version: string, extra: Partial<AddonManifest> = {}) =>
  manifest({
    id: "example.standup-notes",
    name: "Standup Notes",
    version,
    description:
      "Summarizes today’s commits and open changes into a short standup draft.",
    author: { name: "Standup Notes contributors", url: "https://example.com" },
    repository: "https://github.com/example-labs/standup-notes",
    permissions: ["workspace.read", "git.read", "composer.append"],
    contributes: {
      commands: [
        { id: "draft", title: "Draft standup notes", requiresWorkspace: true },
      ],
      panels: [{ id: "notes", title: "Standup", icon: "list-checks" }],
      composerActions: [{ id: "insert", title: "Insert standup", icon: "plus" }],
      composerViews: [],
    },
    settings: [
      {
        id: "format",
        label: "Summary format",
        type: "enum",
        default: "bullets",
        values: ["bullets", "prose"],
      },
      { id: "includeUntracked", label: "Include untracked files", type: "boolean", default: false },
    ],
    ...extra,
  });
const STANDUP_UPDATE = standup("1.3.0", {
  permissions: ["workspace.read", "git.read", "external.open"],
  http: [{ origin: "https://status.example.com", methods: ["GET"], credential: null }],
});
const ticketLens = manifest({
  id: "example.ticket-lens",
  name: "Ticket Lens",
  version: "2.0.1",
  description: "Shows tickets linked to the current branch and lets you comment on them.",
  author: { name: "Ticket Lens maintainers", url: "https://example.com" },
  repository: "https://github.com/northwind-tools/ticket-lens",
  permissions: ["workspace.read", "external.open"],
  http: [
    { origin: "https://api.tickets.example.com", methods: ["GET", "POST"], credential: "api-token" },
    { origin: "https://search.example.com", methods: ["GET"], credential: "search-token" },
  ],
  credentials: [
    { id: "api-token", label: "Ticket API token", origin: "https://api.tickets.example.com", type: "bearer" },
    { id: "search-token", label: "Search token (optional)", origin: "https://search.example.com", type: "bearer" },
  ],
  contributes: {
    commands: [{ id: "open", title: "Open linked tickets", requiresWorkspace: true }],
    panels: [{ id: "tickets", title: "Tickets", icon: "ticket" }],
    composerActions: [],
    composerViews: [{ id: "picker", title: "Ticket picker", icon: "search" }],
  },
  settings: [{ id: "project", label: "Project key", type: "string", default: "" }],
});
const scratch = manifest({
  id: "local.scratch-tools",
  name: "Scratch Tools",
  version: "0.3.0",
  description: "A local package under development: quick scratch notes per project.",
  author: { name: "Scratch Tools authors", url: "https://example.com" },
  repository: "https://github.com/example/scratch-tools",
  permissions: ["workspace.read"],
  http: [{ origin: "https://api.scratch.example.com", methods: ["GET", "PUT"], credential: "token" }],
  credentials: [
    { id: "token", label: "Scratch service token", origin: "https://api.scratch.example.com", type: "bearer" },
  ],
});
const buildWatcher = manifest({
  id: "example.build-watcher",
  name: "Build Watcher",
  version: "0.4.0",
  description: "Watches the last CI run of the current branch.",
  repository: "https://github.com/example-labs/build-watcher",
  permissions: ["workspace.read", "git.read"],
  contributes: {
    commands: [],
    panels: [{ id: "runs", title: "Builds", icon: "activity" }],
    composerActions: [],
    composerViews: [],
  },
});
const formatter = manifest({
  id: "example.legacy-formatter",
  name: "Legacy Formatter",
  version: "1.0.4",
  description: "Formats pasted snippets before they are added to a draft.",
  repository: "https://github.com/example-labs/legacy-formatter",
  permissions: ["composer.append"],
});
const nightly = manifest({
  id: "local.nightly-preview",
  name: "Nightly Preview",
  version: "2.0.0-dev",
  api: "^2.0.0",
  description: "A package built for a newer add-on API than this CodeMux provides.",
  platforms: ["windows-x64"],
});

const catalogSource = (m: AddonManifest, publisher: string): AddonSource => ({
  kind: "catalog",
  publisher,
  repository: m.repository,
});
const listing = (
  m: AddonManifest,
  publisher: string,
  tier: AddonListing["tier"],
  listed: AddonListing["listed"],
): AddonListing => ({ publisher, repository: m.repository, tier, listed });
function compatibility(m: AddonManifest) {
  const api = !m.api.startsWith("^1.");
  const platform = !m.platforms.includes(PLATFORM);
  const reason = api
    ? `Requires plugin API ${m.api}; this CodeMux supports ${HOST_API}`
    : platform
      ? `Built for ${m.platforms.join(", ")}; this device is ${PLATFORM}`
      : null;
  return {
    api: m.api,
    hostApi: HOST_API,
    platforms: m.platforms,
    platform: PLATFORM,
    compatible: reason === null,
    reason,
  };
}

type Item = AddonInstallation & { catalog: AddonListing | null };
let sequence = 0;
function item(
  m: AddonManifest,
  source: AddonSource,
  fields: Partial<Item> = {},
): Item {
  sequence += 1;
  return {
    installationId: `00000000-0000-4000-8000-${String(sequence).padStart(12, "0")}`,
    manifest: m,
    source,
    digest: digest(sequence),
    dataGeneration: `00000000-0000-4000-9000-${String(sequence).padStart(12, "0")}`,
    desiredEnabled: true,
    status: "enabled-idle",
    failure: null,
    previous: null,
    updateAvailable: null,
    catalog: null,
    ...fields,
  };
}

function catalogPlugins(): AddonCatalogPlugin[] {
  const release = (m: AddonManifest, publishedAt: string, platforms = m.platforms) => ({
    version: m.version,
    api: m.api,
    platforms,
    sha256: digest(m.version.length * 11 + m.name.length),
    publishedAt,
    capabilities: {
      permissions: m.permissions,
      http: m.http,
      credentials: m.credentials,
    },
  });
  const focus = manifest({
    id: "example.focus-timer",
    name: "Focus Timer",
    version: "1.0.0",
    description: "A small panel timer that suggests a break between long agent runs.",
    permissions: ["workspace.read"],
    repository: "https://github.com/example-labs/focus-timer",
  });
  const radar = manifest({
    id: "example.release-radar",
    name: "Release Radar",
    version: "0.9.0",
    description: "Lists upcoming releases of the dependencies in this project.",
    platforms: ["windows-x64"],
    repository: "https://github.com/northwind-tools/release-radar",
  });
  return [
    {
      id: standup("1.3.0").id,
      name: "Standup Notes",
      publisher: "Example Labs",
      tier: "official",
      description: standup("1.3.0").description,
      repository: STANDUP_UPDATE.repository,
      readme: "",
      releases: [
        release(STANDUP_UPDATE, "2026-09-18"),
        release(standup("1.2.0"), "2026-08-30"),
        release(standup("1.1.0"), "2026-07-12"),
      ],
    },
    {
      id: focus.id,
      name: focus.name,
      publisher: "Example Labs",
      tier: "official",
      description: focus.description,
      repository: focus.repository,
      readme: "",
      releases: [release(focus, "2026-09-02")],
    },
    {
      id: ticketLens.id,
      name: ticketLens.name,
      publisher: "Northwind Tools",
      tier: "community",
      description: ticketLens.description,
      repository: ticketLens.repository,
      readme: "",
      releases: [release(ticketLens, "2026-09-10")],
    },
    {
      id: radar.id,
      name: radar.name,
      publisher: "Northwind Tools",
      tier: "community",
      description: radar.description,
      repository: radar.repository,
      readme: "",
      releases: [release(radar, "2026-08-21")],
    },
  ];
}
/** Manifests the preview can "install" from the catalog, by ID. */
function catalogManifest(id: string): AddonManifest | null {
  if (id === "example.standup-notes") return STANDUP_UPDATE;
  if (id === "example.focus-timer")
    return manifest({
      id,
      name: "Focus Timer",
      version: "1.0.0",
      description: "A small panel timer that suggests a break between long agent runs.",
      permissions: ["workspace.read"],
      repository: "https://github.com/example-labs/focus-timer",
      contributes: {
        commands: [{ id: "start", title: "Start focus timer", requiresWorkspace: false }],
        panels: [{ id: "timer", title: "Focus", icon: "timer" }],
        composerActions: [],
        composerViews: [],
      },
    });
  if (id === "example.ticket-lens") return ticketLens;
  return null;
}
const PUBLISHERS: Record<string, { publisher: string; tier: "official" | "community" }> = {
  "example.standup-notes": { publisher: "Example Labs", tier: "official" },
  "example.focus-timer": { publisher: "Example Labs", tier: "official" },
  "example.ticket-lens": { publisher: "Northwind Tools", tier: "community" },
  "example.release-radar": { publisher: "Northwind Tools", tier: "community" },
};

function access(m: AddonManifest): AddonAccess {
  return {
    permissions: [...m.permissions],
    http: m.http.map(({ origin, methods }) => ({ origin, methods: [...methods] })),
    credentials: m.credentials.map(({ id, label, origin }) => ({ id, label, origin })),
  };
}
/** Access in `a` that `b` lacks: permissions, methods per origin, credentials. */
function minus(a: AddonAccess, b: AddonAccess): AddonAccess {
  return {
    permissions: a.permissions.filter((p) => !b.permissions.includes(p)),
    http: a.http
      .map(({ origin, methods }) => {
        const other = b.http.find((h) => h.origin === origin)?.methods ?? [];
        return { origin, methods: methods.filter((m) => !other.includes(m)) };
      })
      .filter((h) => h.methods.length > 0),
    credentials: a.credentials.filter(
      (c) => !b.credentials.some((o) => o.id === c.id && o.origin === c.origin),
    ),
  };
}
const empty = (a: AddonAccess) =>
  !a.permissions.length && !a.http.length && !a.credentials.length;

function diagnostics(seed: number, count: number, received: number): AddonDiagnostics {
  const levels = ["log", "info", "log", "warn", "log", "error", "debug"] as const;
  const now = Date.now();
  return {
    received,
    logs: Array.from({ length: count }, (_, i) => ({
      at: now - (count - i) * 47_000,
      level: levels[(i * seed) % levels.length],
      bytes: 40 + ((i * 37 * seed) % 900),
    })),
  };
}

export function addonMockHandlers(): Record<string, Handler> {
  const mode = readMode();
  let paused = mode === "interrupted";
  let developerMode = false;
  let registryFailed = mode === "registry-error";
  let interrupted: string[] = mode === "interrupted" ? ["example.standup-notes"] : [];
  const installed: Item[] = [];
  const credentialStates: Record<string, Record<string, AddonCredentialState>> = {};
  const settings: Record<string, Record<string, unknown>> = {};
  const reviews = new Map<string, AddonReview>();
  if (mode && mode !== "registry-error") {
    installed.push(
      item(standup("1.2.0"), catalogSource(STANDUP_UPDATE, "Example Labs"), {
        status: mode === "interrupted" ? "enabled-idle" : "enabled-running",
        failure:
          mode === "interrupted"
            ? "CodeMux closed while this add-on was starting. Add-ons are paused; review it before resuming."
            : null,
        previous: { manifest: standup("1.1.0") },
        updateAvailable: "1.3.0",
        catalog: listing(STANDUP_UPDATE, "Example Labs", "official", true),
      }),
      item(ticketLens, catalogSource(ticketLens, "Northwind Tools"), {
        catalog: listing(ticketLens, "Northwind Tools", "community", true),
      }),
      item(scratch, { kind: "local", identity: "scratch-tools-local" }, {
        desiredEnabled: false,
        status: "installed-disabled",
      }),
      item(buildWatcher, catalogSource(buildWatcher, "Example Labs"), {
        status: "failed-disabled",
        failure: "Plugin host stopped: Plugin CPU deadline exceeded",
        catalog: listing(buildWatcher, "Example Labs", "community", null),
      }),
      item(formatter, catalogSource(formatter, "Example Labs"), {
        desiredEnabled: false,
        status: "blocked-disabled",
        failure:
          "Catalog block: Sends pasted snippets to an undeclared service",
        catalog: listing(formatter, "Example Labs", "community", false),
      }),
      // Installed on a newer CodeMux, so it stays meant to run.
      item(nightly, { kind: "local", identity: "nightly-local" }, {
        status: "incompatible-disabled",
        failure: compatibility(nightly).reason,
      }),
    );
    credentialStates[ticketLens.id] = {
      "api-token": "saved",
      "search-token": "not-configured",
    };
    credentialStates[scratch.id] = { token: "session-only" };
    settings["example.standup-notes"] = { format: "prose", includeUntracked: true };
    settings[ticketLens.id] = { project: "DEMO" };
  }
  const find = (id: unknown) => {
    const found = installed.find((i) => i.manifest.id === id);
    if (!found) throw fail("PLUGIN_STOPPED", "Add-on is not installed");
    return found;
  };
  const registryError = () =>
    fail(
      "STORAGE_UNAVAILABLE",
      `The add-on registry at ${DATA_DIR} could not be opened: file is not a database. Reset it to start with no add-ons; the current files are kept as a backup.`,
    );
  const needsRegistry = () => {
    if (registryFailed) throw registryError();
  };
  const review = (
    m: AddonManifest,
    publisher: string,
    tier: "official" | "community",
  ): AddonReview => {
    const current = installed.find((i) => i.manifest.id === m.id) ?? null;
    const added = current ? minus(access(m), access(current.manifest)) : access(m);
    const removed = current
      ? minus(access(current.manifest), access(m))
      : { permissions: [], http: [], credentials: [] };
    const value: AddonReview = {
      token: `mock-review-${++sequence}`,
      manifest: m,
      digest: digest(sequence + 40),
      source: catalogSource(m, publisher),
      replacesSource: false,
      expandsAccess: !empty(added),
      compressedBytes: 18_432 + m.name.length * 1_024,
      development: false,
      retainedData: null,
      installed: current && {
        version: current.manifest.version,
        digest: current.digest,
        source: current.source,
        desiredEnabled: current.desiredEnabled,
        capabilities: {
          permissions: current.manifest.permissions,
          http: current.manifest.http,
          credentials: current.manifest.credentials,
        },
      },
      added,
      removed,
      catalog: { publisher, repository: m.repository, tier, listed: true },
    };
    reviews.set(value.token, value);
    return value;
  };
  const catalogReview = (target: string): AddonReview => {
    let id = target;
    let version: string | null = null;
    const link = /^https:\/\/codemux\.org\/addons\/([a-z0-9.-]+)(?:\?version=(.+))?$/.exec(target);
    if (link) {
      id = link[1];
      version = link[2] ? decodeURIComponent(link[2]) : null;
    } else if (!/^[a-z0-9-]+\.[a-z0-9-]+$/.test(target)) {
      throw fail("INVALID_MESSAGE", "Use a catalog ID or a codemux.org add-on install link");
    }
    if (version !== null && !/^\d+\.\d+\.\d+$/.test(version))
      throw fail("INVALID_MESSAGE", "The install link version is not a semantic version");
    if (id === formatter.id)
      throw fail("PERMISSION_DENIED", "This release is blocked: Sends pasted snippets to an undeclared service");
    const plugin = catalogPlugins().find((p) => p.id === id);
    if (!plugin) throw fail("INVALID_MESSAGE", `${id} is not listed in the add-on catalog`);
    if (version !== null && !plugin.releases.some((r) => r.version === version))
      throw fail("INVALID_MESSAGE", `Version ${version} of ${id} is not listed in the add-on catalog`);
    if (id === "example.release-radar")
      throw fail(
        "INCOMPATIBLE_API",
        version
          ? `Version ${version} of ${id} is not available for Linux x64`
          : `No release of ${id} is available for Linux x64`,
      );
    const m = catalogManifest(id);
    if (!m) throw fail("INVALID_MESSAGE", `${id} is not listed in the add-on catalog`);
    const current = installed.find((i) => i.manifest.id === id);
    const wanted = version ?? m.version;
    if (current && wanted === current.manifest.version)
      throw fail("INVALID_MESSAGE", `${m.name} ${wanted} is already installed`);
    if (current && wanted < current.manifest.version)
      throw fail("INVALID_MESSAGE", "Downgrades require the recorded rollback action");
    const { publisher, tier } = PUBLISHERS[id];
    return review(wanted === m.version ? m : { ...m, version: wanted }, publisher, tier);
  };
  const catalog = (): AddonCatalogBrowse => {
    if (!mode)
      return {
        snapshot: null,
        stale: true,
        error:
          "The catalog is unavailable in the browser preview. Use the desktop app to install packages.",
      };
    const hours = mode === "offline" ? 74 : 2;
    return {
      snapshot: {
        fetchedAt: Math.floor(Date.now() / 1000) - hours * 3600,
        catalog: { revision: mode === "offline" ? 41 : 42, plugins: catalogPlugins() },
      },
      stale: mode === "offline",
      error: mode === "offline" ? "Add-on download failed" : null,
    };
  };

  return {
    addon_inventory: () =>
      registryFailed
        ? {
            paused: true,
            installed: [],
            error: registryError().message,
            warnings: [],
            developerMode: false,
            developmentPackage: null,
            credentialStates: {},
            registryError: { path: DATA_DIR, cause: "file is not a database" },
            interruptedActivations: [],
          }
        : {
            paused,
            installed: installed.map((i) => ({ ...i, compatibility: compatibility(i.manifest) })),
            error: null,
            warnings: [],
            developerMode,
            developmentPackage: null,
            credentialStates,
            registryError: null,
            interruptedActivations: paused ? interrupted : [],
          },
    addon_subscribe: () => {
      needsRegistry();
      return null;
    },
    addon_context_changed: () => null,
    addon_composer_register: () => null,
    addon_composer_closed: () => null,
    addon_pause_all: () => {
      needsRegistry();
      paused = true;
      return null;
    },
    addon_resume: () => {
      needsRegistry();
      paused = false;
      interrupted = [];
      for (const i of installed)
        if (i.failure?.startsWith("CodeMux closed")) i.failure = null;
      return null;
    },
    addon_registry_reset: () => {
      if (!registryFailed)
        throw fail("INVALID_MESSAGE", "The add-on registry has not failed to open; nothing was reset");
      registryFailed = false;
      paused = false;
      return `${DATA_DIR}-backup-20260923T101500Z`;
    },
    addon_developer_mode: (args) => {
      needsRegistry();
      developerMode = !!args.enabled;
      return null;
    },
    addon_catalog: () => {
      needsRegistry();
      return catalog();
    },
    addon_catalog_review: (args) => {
      needsRegistry();
      if (!mode)
        throw fail(
          "NETWORK_DENIED",
          "The catalog is unavailable in the browser preview. Use the desktop app to install packages.",
        );
      return catalogReview(String(args.target ?? "").trim());
    },
    addon_check_update: (args) => {
      const current = find(args.id);
      if (current.source.kind !== "catalog")
        throw fail("INVALID_MESSAGE", "Only catalog installations can check for catalog updates");
      if (mode === "offline") throw fail("NETWORK_DENIED", "Add-on download failed");
      if (!current.updateAvailable)
        return { upToDate: true, installedVersion: current.manifest.version, availableVersion: null, review: null };
      const next = catalogReview(current.manifest.id);
      return {
        upToDate: false,
        installedVersion: current.manifest.version,
        availableVersion: next.manifest.version,
        review: next,
      };
    },
    addon_cancel_review: (args) => {
      reviews.delete(String(args.token));
      return null;
    },
    addon_accept_review: (args) => {
      const pending = reviews.get(String(args.token));
      if (!pending) throw fail("INVALID_MESSAGE", "The review expired; open it again");
      reviews.delete(pending.token);
      const enable = !!args.enable;
      const current = installed.find((i) => i.manifest.id === pending.manifest.id);
      if (current) {
        current.previous = { manifest: current.manifest };
        current.manifest = pending.manifest;
        current.digest = pending.digest;
        current.desiredEnabled = enable || current.desiredEnabled;
        current.status = current.desiredEnabled ? "enabled-idle" : "installed-disabled";
        current.updateAvailable = null;
        current.failure = null;
        return current;
      }
      const created = item(pending.manifest, pending.source, {
        desiredEnabled: enable,
        status: enable ? "enabled-idle" : "installed-disabled",
        catalog: pending.catalog ?? null,
      });
      installed.push(created);
      if (pending.manifest.credentials.length)
        credentialStates[pending.manifest.id] = Object.fromEntries(
          pending.manifest.credentials.map((c) => [c.id, "not-configured" as const]),
        );
      return created;
    },
    addon_enable: (args) => {
      const current = find(args.id);
      if (["blocked-disabled", "incompatible-disabled", "removing"].includes(current.status))
        throw fail("PERMISSION_DENIED", "This release cannot be enabled");
      current.desiredEnabled = true;
      current.status = "enabled-idle";
      current.failure = null;
      return null;
    },
    addon_disable: (args) => {
      const current = find(args.id);
      current.desiredEnabled = false;
      // Like the host, compatibility still decides the reported status.
      current.status = compatibility(current.manifest).compatible
        ? "installed-disabled"
        : "incompatible-disabled";
      return null;
    },
    addon_remove: (args) => {
      const index = installed.findIndex((i) => i.manifest.id === args.id);
      if (index < 0) throw fail("PLUGIN_STOPPED", "Add-on is not installed");
      installed.splice(index, 1);
      delete credentialStates[String(args.id)];
      return [];
    },
    addon_rollback: (args) => {
      const current = find(args.id);
      if (!current.previous) throw fail("INVALID_MESSAGE", "No previous release is recorded");
      const newer = current.manifest.version;
      current.manifest = current.previous.manifest;
      current.previous = null;
      current.updateAvailable = newer;
      return null;
    },
    addon_retry_cleanup: () => [],
    addon_settings_get: (args) => {
      const current = find(args.id);
      return {
        ...Object.fromEntries(current.manifest.settings.map((s) => [s.id, s.default])),
        ...settings[current.manifest.id],
      };
    },
    addon_settings_set: (args) => {
      settings[find(args.id).manifest.id] = { ...(args.settings as Record<string, unknown>) };
      return null;
    },
    addon_credential_set: (args) => {
      const current = find(args.id);
      (credentialStates[current.manifest.id] ??= {})[String(args.credentialId)] =
        args.sessionOnly ? "session-only" : "saved";
      return null;
    },
    addon_credential_clear: (args) => {
      const current = find(args.id);
      if (!current.manifest.credentials.some((c) => c.id === args.credentialId))
        throw fail("PERMISSION_DENIED", "Credential was not declared");
      (credentialStates[current.manifest.id] ??= {})[String(args.credentialId)] = "not-configured";
      return [];
    },
    addon_diagnostics: (args) => {
      const current = find(args.id);
      if (current.manifest.id === buildWatcher.id) return diagnostics(3, 64, 212);
      if (current.status === "enabled-running") return diagnostics(1, 14, 14);
      return { received: 0, logs: [] };
    },
  };
}
