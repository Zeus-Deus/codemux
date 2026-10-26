import * as NodeCrypto from "node:crypto";

// Leave headroom below the public host limits (2000 nodes / 128000 UTF-8 bytes).
export const TREE_LIMITS = Object.freeze({ nodes: 1800, bytes: 120000 });
const id = (...parts) =>
  `n-${NodeCrypto.createHash("sha256").update(JSON.stringify(parts)).digest("hex")}`;
const compare = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
const timestamp = (value) => Date.parse(value ?? "") || 0;
const section = (thread, now) => {
  if (thread.pinnedAt) return "pinned";
  if (thread.isRunning || thread.needsAttention) return "active";
  if (timestamp(thread.snoozedUntil) > now) return "snoozed";
  if (
    thread.settledOverride === "settled" ||
    (thread.settledAt && thread.settledOverride !== "active")
  )
    return "settled";
  return "active";
};
const unique = (values) => [...new Map(values.map((value) => [value.id, value])).values()];

export function buildTree({ projects, threads, now, quietAfterDays = 3, emptyProjects = true }) {
  const nodes = [];
  const emitted = new Set();
  let bytes = Buffer.byteLength(JSON.stringify({ nodes }));
  const append = (path) => {
    const missing = path.filter((node) => !emitted.has(node.id));
    const extra = missing.reduce(
      (sum, node) => sum + Buffer.byteLength(JSON.stringify(node)) + 1,
      0,
    );
    if (nodes.length + missing.length > TREE_LIMITS.nodes || bytes + extra > TREE_LIMITS.bytes)
      return false;
    for (const node of missing) {
      nodes.push(node);
      emitted.add(node.id);
    }
    bytes += extra;
    return true;
  };
  const byProject = new Map();
  for (const thread of unique(threads)) {
    if (
      thread.archivedAt ||
      (thread.creationSource === "provider" && thread.lineage?.relationshipToParent === "subagent")
    )
      continue;
    const list = byProject.get(thread.projectId) ?? [];
    list.push(thread);
    byProject.set(thread.projectId, list);
  }
  const groups = unique(projects)
    .map((project) => {
      const members = byProject.get(project.id) ?? [];
      const lastActivity = Math.max(
        timestamp(project.updatedAt),
        ...members.map((t) => timestamp(t.updatedAt)),
      );
      const quiet =
        !members.some((t) => ["active", "pinned"].includes(section(t, now))) &&
        lastActivity < now - quietAfterDays * 86400000;
      return { project, members, quiet };
    })
    .sort(
      (a, b) =>
        Number(a.quiet) - Number(b.quiet) ||
        compare(a.project.title, b.project.title) ||
        compare(a.project.id, b.project.id),
    );
  const quietGroup = {
    id: id("quiet"),
    kind: "group",
    title: "Quiet projects",
    collapsedByDefault: true,
  };
  for (const { project, members, quiet } of groups) {
    if (!members.length && !emptyProjects) continue;
    const heading = {
      id: id("project", project.id),
      kind: "project",
      projectId: project.id,
      ...(quiet ? { parentId: quietGroup.id } : {}),
    };
    const prefix = quiet ? [quietGroup, heading] : [heading];
    if (!members.length) {
      append(prefix);
      continue;
    }
    for (const name of ["pinned", "active", "snoozed", "settled"]) {
      const list = members
        .filter((t) => section(t, now) === name)
        .sort(
          (a, b) =>
            timestamp(b.pinnedAt ?? b.updatedAt) - timestamp(a.pinnedAt ?? a.updatedAt) ||
            compare(a.id, b.id),
        );
      if (!list.length) continue;
      const shelf = ["snoozed", "settled"].includes(name)
        ? {
            id: id("shelf", project.id, name),
            kind: "group",
            title: name === "snoozed" ? "Snoozed" : "Settled",
            parentId: heading.id,
            collapsedByDefault: true,
          }
        : null;
      const base = shelf ? [...prefix, shelf] : prefix;
      const byId = new Map(list.map((t) => [t.id, t]));
      const parents = new Map();
      for (const t of list) {
        const parentId = t.lineage?.relationshipToParent
          ? t.lineage.parentThreadId
          : t.launchedFrom?.threadId;
        if (["pinned", "active"].includes(name) && byId.has(parentId)) parents.set(t.id, parentId);
      }
      const paths = new Map();
      for (const t of list) {
        const chain = [];
        const seen = new Set([t.id]);
        let cursor = parents.get(t.id);
        // Flatten cyclic, cyclic-dependent or excessively deep paths at the section root.
        while (cursor && !seen.has(cursor) && chain.length < 11) {
          seen.add(cursor);
          chain.push(cursor);
          cursor = parents.get(cursor);
        }
        paths.set(t.id, cursor ? [] : chain.toReversed());
      }
      for (const t of list) {
        const chain = [...paths.get(t.id), t.id];
        const path = [...base];
        let parentId = base.at(-1).id;
        for (const threadId of chain) {
          const node = { id: id("thread", threadId), parentId, kind: "thread", threadId };
          path.push(node);
          parentId = node.id;
        }
        append(path);
      }
    }
  }
  return { nodes };
}
