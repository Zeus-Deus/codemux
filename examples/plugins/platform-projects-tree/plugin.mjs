import { buildTree } from "./tree.mjs";

const SCAN = { projects: 200, threads: 6000, calls: 80, bytes: 4000000 };

export function activate(ctx) {
  ctx.trees.register("projects", async (invocation) => {
    let calls = 0;
    let bytes = 0;
    let truncated = false;
    const collect = async (read, field, max) => {
      const values = new Map();
      let cursor = 0;
      for (; calls < SCAN.calls && bytes < SCAN.bytes && values.size < max;) {
        invocation.signal.throwIfAborted();
        calls++;
        const page = await read({ cursor, limit: 100 });
        bytes += Buffer.byteLength(JSON.stringify(page));
        if (bytes > SCAN.bytes) {
          truncated = true;
          break;
        }
        for (const item of page[field]) {
          if (values.size >= max) {
            truncated = true;
            break;
          }
          values.set(item.id, item);
        }
        if (page.nextCursor === null) return [...values.values()];
        if (!Number.isSafeInteger(page.nextCursor) || page.nextCursor <= cursor) {
          truncated = true;
          break;
        }
        cursor = page.nextCursor;
      }
      truncated = true;
      return [...values.values()];
    };
    const emptyProjects = (await ctx.settings.get("emptyProjects")) !== false;
    const days = await ctx.settings.get("quietAfterDays");
    const quietAfterDays =
      typeof days === "number" && Number.isFinite(days) ? Math.min(365, Math.max(1, days)) : 3;
    const projects = await collect((page) => ctx.projects.list(page), "projects", SCAN.projects);
    const threads = [];
    for (const project of projects) {
      if (threads.length >= SCAN.threads || calls >= SCAN.calls || bytes >= SCAN.bytes) {
        truncated = true;
        break;
      }
      threads.push(
        ...(await collect(
          (page) => ctx.threads.list({ ...page, projectId: project.id }),
          "threads",
          SCAN.threads - threads.length,
        )),
      );
    }
    const snapshot = buildTree({
      projects,
      threads,
      now: Date.now(),
      quietAfterDays,
      emptyProjects,
    });
    if (
      truncated ||
      snapshot.nodes.filter((n) => n.kind === "thread").length <
        threads.filter((t) => !t.archivedAt).length
    ) {
      ctx.log(
        "info",
        "Projects snapshot is bounded; unplaced threads remain available in the host sidebar.",
      );
    }
    return snapshot;
  });
}
