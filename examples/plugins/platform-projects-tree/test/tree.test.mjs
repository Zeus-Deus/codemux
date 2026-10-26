import * as NodeAssert from "node:assert/strict";
import * as NodeTest from "node:test";
import { buildTree, TREE_LIMITS } from "../tree.mjs";
import { activate } from "../plugin.mjs";

const now = Date.parse("2026-10-23T12:00:00Z");
const project = (id = "p", extra = {}) => ({
  id,
  title: id,
  updatedAt: "2026-01-01T00:00:00Z",
  ...extra,
});
const thread = (id, extra = {}) => ({
  id,
  projectId: "p",
  updatedAt: "2026-10-23T00:00:00Z",
  archivedAt: null,
  pinnedAt: null,
  snoozedUntil: null,
  settledAt: null,
  settledOverride: null,
  lineage: { parentThreadId: null, relationshipToParent: null },
  ...extra,
});
const make = (threads, extra = {}) => buildTree({ projects: [project()], threads, now, ...extra });
const nodeFor = (tree, id) => tree.nodes.find((n) => n.threadId === id || n.projectId === id);
const parent = (tree, id) => tree.nodes.find((n) => n.id === nodeFor(tree, id)?.parentId);
const validate = (tree) => {
  NodeAssert.ok(tree.nodes.length <= TREE_LIMITS.nodes);
  NodeAssert.ok(Buffer.byteLength(JSON.stringify(tree)) <= TREE_LIMITS.bytes);
  const ids = new Set(tree.nodes.map((n) => n.id));
  NodeAssert.equal(ids.size, tree.nodes.length);
  for (const n of tree.nodes) {
    NodeAssert.match(n.id, /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/);
    const seen = new Set([n.id]);
    let cursor = n;
    while (cursor.parentId) {
      NodeAssert.ok(ids.has(cursor.parentId));
      NodeAssert.ok(!seen.has(cursor.parentId));
      seen.add(cursor.parentId);
      cursor = tree.nodes.find((p) => p.id === cursor.parentId);
    }
    NodeAssert.ok(seen.size <= 16);
  }
};

NodeTest.test(
  "pinned leads even if parked; attention/running defeat parking; provider proof is specific",
  () => {
    const tree = make([
      thread("settled", { settledAt: "2026-10-01" }),
      thread("snoozed", { snoozedUntil: "2027-01-01" }),
      thread("pinned:/", {
        pinnedAt: "2026-01-01",
        settledAt: "2026-01-01",
        snoozedUntil: "2027-01-01",
      }),
      thread("attention", { needsAttention: true, settledAt: "2026-01-01" }),
      thread("running", { isRunning: true, snoozedUntil: "2027-01-01" }),
      thread("native", {
        creationSource: "provider",
        lineage: { parentThreadId: "attention", relationshipToParent: "subagent" },
      }),
      thread("owned", {
        creationSource: "mcp",
        lineage: { parentThreadId: "attention", relationshipToParent: "subagent" },
      }),
      thread("unknown", {
        lineage: { parentThreadId: "attention", relationshipToParent: "subagent" },
      }),
      thread("archived", { archivedAt: "2026-01-01" }),
    ]);
    NodeAssert.equal(tree.nodes.find((n) => n.kind === "thread").threadId, "pinned:/");
    NodeAssert.equal(parent(tree, "settled").title, "Settled");
    NodeAssert.equal(parent(tree, "snoozed").title, "Snoozed");
    NodeAssert.equal(parent(tree, "snoozed").collapsedByDefault, true);
    for (const id of ["pinned:/", "attention", "running"])
      NodeAssert.equal(parent(tree, id).kind, "project");
    for (const id of ["native", "archived"]) NodeAssert.equal(nodeFor(tree, id), undefined);
    for (const id of ["owned", "unknown"]) NodeAssert.equal(parent(tree, id).threadId, "attention");
    validate(tree);
  },
);

NodeTest.test(
  "nests forks and independent launches only within section/project, breaking cycles and deep chains",
  () => {
    const threads = [
      thread("a"),
      thread("fork", { lineage: { parentThreadId: "a", relationshipToParent: "fork" } }),
      thread("launch", { launchedFrom: { threadId: "fork" } }),
      thread("parked", { settledAt: "2026-01-01" }),
      thread("root", { launchedFrom: { threadId: "parked" } }),
      thread("cycle1", { launchedFrom: { threadId: "cycle2" } }),
      thread("cycle2", { launchedFrom: { threadId: "cycle1" } }),
      thread("foreign", { projectId: "other", launchedFrom: { threadId: "a" } }),
    ];
    for (let i = 0; i < 100; i++)
      threads.push(thread(`deep${i}`, { launchedFrom: { threadId: i ? `deep${i - 1}` : "a" } }));
    const tree = make(threads, { projects: [project(), project("other")] });
    NodeAssert.equal(parent(tree, "fork").threadId, "a");
    NodeAssert.equal(parent(tree, "launch").threadId, "fork");
    for (const id of ["root", "cycle1", "cycle2", "foreign"])
      NodeAssert.equal(parent(tree, id).kind, "project");
    validate(tree);
  },
);

NodeTest.test(
  "quiet projects fold only without live work; empty setting and input order are deterministic",
  () => {
    const input = {
      projects: [project("empty"), project("old"), project()],
      threads: [
        thread("live"),
        thread("old", { projectId: "old", updatedAt: "2026-01-01", settledAt: "2026-01-01" }),
      ],
      now,
      quietAfterDays: 3,
      emptyProjects: true,
    };
    const tree = buildTree(input);
    NodeAssert.equal(parent(tree, "old").title, "Quiet projects");
    NodeAssert.equal(parent(tree, "empty").collapsedByDefault, true);
    NodeAssert.deepEqual(
      buildTree({
        ...input,
        projects: input.projects.toReversed(),
        threads: input.threads.toReversed(),
      }),
      tree,
    );
    NodeAssert.equal(nodeFor(buildTree({ ...input, emptyProjects: false }), "empty"), undefined);
    validate(tree);
  },
);

NodeTest.test(
  "count and actual UTF8 bytes remain bounded with complete ancestor chains and duplicate metadata",
  () => {
    const threads = Array.from({ length: 5000 }, (_, i) =>
      thread(`${"長".repeat(120)}:${i}`, {
        launchedFrom: { threadId: i ? `${"長".repeat(120)}:${i - 1}` : "missing" },
      }),
    );
    const tree = make([...threads, threads[0]], { projects: [project(), project()] });
    validate(tree);
    NodeAssert.ok(tree.nodes.length < threads.length);
    NodeAssert.equal(
      new Set(tree.nodes.filter((n) => n.kind === "thread").map((n) => n.threadId)).size,
      tree.nodes.filter((n) => n.kind === "thread").length,
    );
  },
);

NodeTest.test(
  "paged public facade deduplicates, bounds hostile cursors, and propagates read revocation",
  async () => {
    let handler;
    let calls = 0;
    let revoked = false;
    await activate({
      capabilities: ["settings", "threads.read", "workflow.query", "ui.native-trees", "ui.changes"],
      trees: {
        register: (_id, fn) => {
          handler = fn;
        },
      },
      settings: { get: async () => null },
      projects: { list: async () => ({ projects: [project(), project()], nextCursor: null }) },
      threads: {
        list: async () => {
          if (revoked) throw new Error("permission_denied");
          calls++;
          return { threads: [thread("a")], nextCursor: 0 };
        },
      },
      log: () => {},
    });
    const invocation = { signal: new AbortController().signal };
    NodeAssert.equal(
      (await handler(invocation)).nodes.filter((n) => n.kind === "thread").length,
      1,
    );
    NodeAssert.equal(calls, 1);
    revoked = true;
    await NodeAssert.rejects(() => handler(invocation), /permission_denied/);
  },
);

NodeTest.test("parked parents never acquire nested children, including on the same shelf", () => {
  const tree = make([
    thread("parked-parent", { settledAt: "2026-01-01" }),
    thread("parked-child", {
      settledAt: "2026-01-01",
      launchedFrom: { threadId: "parked-parent" },
    }),
  ]);
  NodeAssert.equal(parent(tree, "parked-child").title, "Settled");
});

NodeTest.test(
  "separate host snapshots with identical local IDs never share cached metadata or parentage",
  () => {
    const child = thread("child", { launchedFrom: { threadId: "parent" } });
    const a = make([thread("parent"), child]);
    const b = make([child]);
    NodeAssert.equal(parent(a, "child").threadId, "parent");
    NodeAssert.equal(parent(b, "child").kind, "project");
    // Local IDs are stable; installation/environment scope is added by the host.
    NodeAssert.equal(nodeFor(a, "p").id, nodeFor(b, "p").id);
  },
);
