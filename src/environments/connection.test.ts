import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { RegisteredDevice } from "@/remote/device-registry";
import { DeviceRegistryError } from "@/remote/device-registry";
import { encodeFrame, FrameDecoder, KIND_TEXT } from "@/remote/iroh-codec";
import type { IrohByteStream, IrohDialer } from "@/remote/iroh-transport";
import type { AppStateSnapshot, PaneStatus, RevisionedDelta, WorkspaceSnapshot } from "@/tauri/types";

import { openEnvironmentConnection } from "./connection";
import { useEnvironmentsStore, type EnvironmentView } from "./store";

const enc = new TextEncoder();
const dec = new TextDecoder();

const device: RegisteredDevice = {
  id: "7",
  deviceId: "env-ai-node",
  nodeId: "node-ai-node",
  name: "ai-node",
  platform: "linux",
  lastSeenAt: new Date().toISOString(),
};

function workspace(title: string): WorkspaceSnapshot {
  return {
    workspace_id: title, title, workspace_type: "standard", cwd: "/tmp/project",
    git_branch: null, git_ahead: 0, git_behind: 0, git_additions: 0,
    git_deletions: 0, git_changed_files: 0, notification_count: 3,
    notifications_muted: false, latest_agent_state: null, worktree_path: null,
    project_root: null, pr_number: null, pr_state: null, pr_url: null,
    linked_issue: null, tabs: [], active_tab_id: "", active_surface_id: "", surfaces: [],
  };
}

function snapshot(revision: number, titles: string[]): AppStateSnapshot {
  return {
    schema_version: 1,
    snapshot_revision: revision,
    snapshot_instance: "instance-1",
    active_workspace_id: titles[0] ?? "",
    workspaces: titles.map(workspace),
    terminal_sessions: [], browser_sessions: [], agent_browser_sessions: [],
    notifications: [], detected_ports: [], pane_statuses: {},
    persistence: {
      schema_version: 1, stores_layout_metadata: true,
      stores_terminal_metadata: true, stores_live_process_state: true,
    },
    config: {
      config_version: 1, default_shell: null, theme_source: "default",
      linux_first: false, notification_sound_enabled: true,
      ai_commit_message_enabled: false, ai_commit_message_cli: null,
      ai_commit_message_model: null, ai_resolver_enabled: false,
      ai_resolver_cli: null, ai_resolver_model: null, ai_resolver_strategy: "auto",
    },
  };
}

/** A machine at the far end of the stream: answers the account handshake,
 *  records `listen` frames and serves `get_app_state`. */
class FakeMachine implements IrohByteStream {
  readonly frames: Array<Record<string, unknown>> = [];
  closed = false;
  private readonly decoder = new FrameDecoder();
  private readonly queued: Uint8Array[] = [];
  private waiting: ((chunk: Uint8Array | null) => void) | null = null;

  constructor(
    private readonly reply: (frame: Record<string, unknown>) => Record<string, unknown> | null,
  ) {}

  async write(data: Uint8Array): Promise<void> {
    this.decoder.push(data);
    for (let frame = this.decoder.next(); frame; frame = this.decoder.next()) {
      const json = JSON.parse(dec.decode(frame.payload)) as Record<string, unknown>;
      this.frames.push(json);
      const answer = this.reply(json);
      if (answer) this.send(answer);
    }
  }

  read(): Promise<Uint8Array | null> {
    const chunk = this.queued.shift();
    if (chunk) return Promise.resolve(chunk);
    if (this.closed) return Promise.resolve(null);
    return new Promise((resolve) => {
      this.waiting = resolve;
    });
  }

  close(): void {
    this.closed = true;
    this.waiting?.(null);
    this.waiting = null;
  }

  send(frame: Record<string, unknown>): void {
    const bytes = encodeFrame(KIND_TEXT, enc.encode(JSON.stringify(frame)));
    const waiting = this.waiting;
    this.waiting = null;
    if (waiting) waiting(bytes);
    else this.queued.push(bytes);
  }
}

async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

function collect() {
  const patches: Array<Partial<Omit<EnvironmentView, "device">>> = [];
  useEnvironmentsStore.getState().setCatalog([device]);
  return {
    patches,
    onUpdate: (patch: (typeof patches)[number]) => {
      patches.push(patch);
      useEnvironmentsStore.getState().update(device.deviceId, patch);
    },
  };
}

const grant = { nodeId: device.nodeId, grant: "v1.grant", relayUrlHint: null, expiresAt: "" };

beforeEach(() => useEnvironmentsStore.getState().reset());
afterEach(() => vi.useRealTimers());

function state() {
  return useEnvironmentsStore.getState().environments[device.deviceId].appState;
}

function machineConnection(initial: AppStateSnapshot | null) {
  const requests: Array<Record<string, unknown>> = [];
  const machine = new FakeMachine((frame) => {
    if (frame.t === "hello-account") return { t: "welcome", session_id: "s1" };
    if (frame.t === "invoke" && frame.cmd === "get_app_state") {
      requests.push(frame);
      if (requests.length === 1 && initial) return { t: "ok", id: frame.id, data: initial };
    }
    return null;
  });
  const { patches, onUpdate } = collect();
  const connection = openEnvironmentConnection({
    device, registry: { connectDevice: async () => grant },
    dialer: { dial: async () => machine }, onUpdate,
  });
  return {
    machine, connection, requests, patches,
    delta: (payload: RevisionedDelta) => machine.send({
      t: "event", event: "app-state-delta", payload: { instance: "instance-1", ...payload },
    }),
    heartbeat: (revision: number, instance = "instance-1") => machine.send({
      t: "event", event: "app-state-revision", payload: { revision, instance },
    }),
    reply: (index: number, data: AppStateSnapshot) => machine.send({ t: "ok", id: requests[index].id, data }),
  };
}

describe("openEnvironmentConnection", () => {
  it("handshakes with a grant, reads the machine's state and follows its changes", async () => {
    const machine = new FakeMachine((frame) => {
      if (frame.t === "hello-account") return { t: "welcome", session_id: "s1" };
      if (frame.t === "invoke" && frame.cmd === "get_app_state") {
        return { t: "ok", id: frame.id, data: snapshot(1, ["auth-refactor"]) };
      }
      return null;
    });
    const dialer: IrohDialer = { dial: vi.fn(async () => machine) };
    const registry = { connectDevice: vi.fn(async () => grant) };
    const { patches, onUpdate } = collect();

    const connection = openEnvironmentConnection({ device, registry, dialer, onUpdate });
    await settle();

    expect(registry.connectDevice).toHaveBeenCalledWith("7", expect.any(String));
    expect(dialer.dial).toHaveBeenCalledWith({ nodeId: "node-ai-node", relayUrl: null });
    expect(machine.frames[0]).toMatchObject({ t: "hello-account", grant: "v1.grant" });
    for (const event of ["app-state-changed", "app-state-delta", "app-state-revision"]) {
      expect(machine.frames).toContainEqual({ t: "listen", event });
    }
    expect(patches).toContainEqual({ status: "connected", error: null });
    expect(patches).toContainEqual({ appState: snapshot(1, ["auth-refactor"]) });

    machine.send({ t: "event", event: "app-state-changed", payload: snapshot(2, ["auth-refactor", "new-chat"]) });
    await settle();
    expect(patches[patches.length - 1]).toEqual({ appState: snapshot(2, ["auth-refactor", "new-chat"]) });

    connection.close();
    await settle();
    expect(machine.closed).toBe(true);
  });

  it.each<PaneStatus>(["working", "permission"])("applies a delta-only %s clear and rejects stale deliveries", async (status) => {
    const initial = { ...snapshot(10, ["chat"]), pane_statuses: { pane: status } };
    const peer = machineConnection(initial);
    await settle();
    peer.delta({ revision: 11, delta: { domain: "pane_status", pane_id: "pane", status: null } });
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 11, pane_statuses: {} });

    peer.machine.send({ t: "event", event: "app-state-changed", payload: initial });
    peer.delta({ revision: 11, delta: { domain: "pane_status", pane_id: "pane", status } });
    await settle();
    expect(state()?.pane_statuses).toEqual({});
    expect(peer.requests).toHaveLength(1);
    peer.connection.close();
  });

  it("clears Review and unread counts on the activation delta without a snapshot", async () => {
    const initial = {
      ...snapshot(10, ["previous", "target"]),
      pane_statuses: { review: "review", working: "working" } satisfies Record<string, PaneStatus>,
    };
    const peer = machineConnection(initial);
    await settle();
    peer.delta({ revision: 11, delta: {
      domain: "active_workspace", workspace_id: "target", previous_workspace_id: "previous",
      last_visited_at: 200, previous_last_visited_at: 199, cleared_review_pane_ids: ["review"],
    } });
    await settle();
    expect(state()).toMatchObject({
      snapshot_revision: 11, active_workspace_id: "target", pane_statuses: { working: "working" },
    });
    expect(state()?.pane_statuses).not.toHaveProperty("review");
    expect(state()?.workspaces[1]).toMatchObject({ notification_count: 0, last_visited_at: 200 });
    expect(peer.requests).toHaveLength(1);
    peer.connection.close();
  });

  it("advances past no-op deltas without treating the next revision as a gap", async () => {
    const peer = machineConnection(snapshot(10, ["chat"]));
    await settle();
    peer.delta({ revision: 11, delta: { domain: "pane_status", pane_id: "absent", status: null } });
    await settle();
    expect(state()?.snapshot_revision).toBe(11);
    peer.delta({ revision: 12, delta: { domain: "pane_status", pane_id: "pane", status: "working" } });
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 12, pane_statuses: { pane: "working" } });
    expect(peer.requests).toHaveLength(1);
    peer.connection.close();
  });

  it("replays a delta that overtakes the initial snapshot response", async () => {
    const peer = machineConnection(null);
    await settle();
    peer.delta({ revision: 11, delta: { domain: "pane_status", pane_id: "pane", status: null } });
    await settle();
    peer.reply(0, { ...snapshot(10, ["chat"]), pane_statuses: { pane: "working" } });
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 11, pane_statuses: {} });
    expect(peer.requests).toHaveLength(1);
    peer.connection.close();
  });

  it("allows a snapshot to close a reordered gap without fetching", async () => {
    const peer = machineConnection(snapshot(10, ["chat"]));
    await settle();
    peer.delta({ revision: 12, delta: { domain: "pane_status", pane_id: "pane", status: null } });
    await settle();
    expect(state()?.snapshot_revision).toBe(10);
    peer.machine.send({ t: "event", event: "app-state-changed", payload: {
      ...snapshot(11, ["chat"]), pane_statuses: { pane: "permission" },
    } });
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 12, pane_statuses: {} });
    expect(peer.requests).toHaveLength(1);
    peer.connection.close();
  });

  it("resyncs a real gap once and replays deltas newer than the fetched baseline", async () => {
    const peer = machineConnection(snapshot(10, ["chat"]));
    await settle();
    peer.delta({ revision: 12, delta: { domain: "pane_status", pane_id: "pane", status: "working" } });
    await vi.waitFor(() => expect(peer.requests).toHaveLength(2));
    peer.delta({ revision: 13, delta: { domain: "pane_status", pane_id: "pane", status: null } });
    peer.heartbeat(13);
    await settle();
    expect(peer.requests).toHaveLength(2);
    peer.reply(1, snapshot(11, ["chat"]));
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 13, pane_statuses: {} });
    expect(peer.requests).toHaveLength(2);
    peer.connection.close();
  });

  it("resyncs on a newer heartbeat and on a lower revision from a restarted backend", async () => {
    const peer = machineConnection(snapshot(500, ["old"]));
    await settle();
    peer.heartbeat(499);
    await settle();
    expect(peer.requests).toHaveLength(1);
    peer.heartbeat(501);
    await settle();
    expect(peer.requests).toHaveLength(2);

    peer.heartbeat(2, "instance-2");
    await settle();
    expect(peer.requests).toHaveLength(3);
    peer.reply(2, { ...snapshot(2, ["restarted"]), snapshot_instance: "instance-2" });
    await settle();
    // An older in-flight response cannot resurrect the previous process.
    peer.reply(1, snapshot(501, ["old"]));
    await settle();
    expect(state()).toMatchObject({ snapshot_instance: "instance-2", snapshot_revision: 2 });
    expect(state()?.workspaces[0].title).toBe("restarted");
    peer.connection.close();
  });

  it("fetches again if the delta buffer overflowed beyond an in-flight snapshot", async () => {
    const peer = machineConnection(null);
    await settle();
    for (let revision = 11; revision <= 75; revision++) {
      peer.delta({ revision, delta: { domain: "pane_status", pane_id: "pane", status: "working" } });
    }
    await settle();
    expect(peer.requests).toHaveLength(1);
    peer.reply(0, snapshot(10, ["old-baseline"]));
    await settle();
    expect(peer.requests).toHaveLength(2);
    peer.reply(1, { ...snapshot(75, ["current"]), pane_statuses: { pane: "working" } });
    await settle();
    expect(state()).toMatchObject({ snapshot_revision: 75, pane_statuses: { pane: "working" } });
    peer.connection.close();
  });

  it("clears a resync failure after a successful retry without reconnecting", async () => {
    const peer = machineConnection(snapshot(10, ["chat"]));
    await settle();
    peer.heartbeat(11);
    await settle();
    peer.machine.send({ t: "err", id: peer.requests[1].id, error: "temporarily unavailable" });
    await settle();
    expect(useEnvironmentsStore.getState().environments[device.deviceId].error).toBe("temporarily unavailable");
    peer.heartbeat(11);
    await settle();
    peer.reply(2, snapshot(11, ["recovered"]));
    await settle();
    expect(state()?.workspaces[0].title).toBe("recovered");
    expect(useEnvironmentsStore.getState().environments[device.deviceId]).toMatchObject({ status: "connected", error: null });
    peer.connection.close();
  });

  it("keeps retrying after a failed resync and ignores responses after close", async () => {
    const peer = machineConnection(snapshot(10, ["chat"]));
    await settle();
    peer.heartbeat(11);
    await settle();
    peer.machine.send({ t: "err", id: peer.requests[1].id, error: "temporarily unavailable" });
    await settle();
    expect(useEnvironmentsStore.getState().environments[device.deviceId].error).toBe("temporarily unavailable");
    peer.heartbeat(11);
    await settle();
    expect(peer.requests).toHaveLength(3);
    peer.connection.close();
    const count = peer.patches.length;
    peer.reply(2, snapshot(11, ["late"]));
    await settle();
    expect(peer.patches).toHaveLength(count);
    expect(state()?.snapshot_revision).toBe(10);
  });

  it("reports a machine that has not approved this device yet", async () => {
    const machine = new FakeMachine((frame) =>
      frame.t === "hello-account" ? { t: "unauthorized", reason: "pending_approval" } : null,
    );
    const { patches, onUpdate } = collect();
    const connection = openEnvironmentConnection({
      device,
      registry: { connectDevice: async () => grant },
      dialer: { dial: async () => machine },
      onUpdate,
    });
    await settle();
    expect(patches).toContainEqual({ status: "pending-approval", error: null });
    connection.close();
  });

  it("reports an offline machine and stops reporting once closed", async () => {
    const { patches, onUpdate } = collect();
    const connection = openEnvironmentConnection({
      device,
      registry: {
        connectDevice: async () => {
          throw new DeviceRegistryError("offline", "That desktop is offline right now.", 409);
        },
      },
      dialer: { dial: vi.fn() },
      onUpdate,
    });
    await settle();
    expect(patches).toContainEqual({ status: "offline", error: "That desktop is offline right now." });
    connection.close();
    const count = patches.length;
    await settle();
    expect(patches).toHaveLength(count);
  });

  it("stops retrying when the account session is gone", async () => {
    const { patches, onUpdate } = collect();
    const connectDevice = vi.fn(async () => {
      throw new DeviceRegistryError("unauthorized", "Sign in again.", 401);
    });
    openEnvironmentConnection({ device, registry: { connectDevice }, dialer: { dial: vi.fn() }, onUpdate });
    await settle();
    expect(patches).toContainEqual({ status: "unauthorized", error: "Sign in again to reach this machine." });
    expect(connectDevice).toHaveBeenCalledOnce();
  });
});
