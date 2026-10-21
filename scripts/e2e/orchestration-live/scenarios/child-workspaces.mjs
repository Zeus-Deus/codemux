// Child workspaces: a coordinator chat in a linked worktree starts two chats
// with thread_launch (no workspace) and delegates one task. Each child must
// become its own workspace, nested under the coordinator, in its folder and
// titled with the task, while the populated main checkout stays as it was.
import { randomUUID } from "node:crypto";

export const name = "child-workspaces";
export const description =
  "A worktree coordinator calls thread_launch twice without a workspace and delegate_task once; " +
  "assert three titled child workspaces nested under it and an unchanged main checkout.";

const CLAUDE_MODEL = process.env.ORCH_LIVE_CLAUDE_MODEL ?? "claude-haiku-4-5";
const CODEX_MODEL = process.env.ORCH_LIVE_CODEX_MODEL ?? "gpt-6-luna";
const TITLES = {
  codex: "CI speed review — Codex",
  claude: "CI speed review — Claude",
  delegated: "Delegated CI check",
};

/** A chat bound to a fresh thread in `workspaceId`, started and sent `text`,
 *  through the paired browser's remote transport. */
async function startChat(ctx, { workspaceId, cwd, provider, model, effort = null, permissionMode = null, text }) {
  const threadId = randomUUID();
  const paneId = await ctx.driver.invoke("agent_chat_create_pane", {
    workspaceId, provider, cwd, launchMode: "new_tab", threadId,
  });
  await ctx.driver.invoke("agent_chat_start_session", {
    paneId, provider, expectedThread: null,
    input: {
      thread_id: threadId, cwd, model, resume_cursor: null,
      permission_mode: permissionMode, effort, context_window: null,
      service_tier: "default", additional_directories: [], env: null,
    },
  });
  if (text) {
    await ctx.driver.invoke("agent_chat_send_turn", {
      provider,
      input: {
        thread_id: threadId, text, display_text: text, skill_ids: [],
        skill_text: null, include_plugins: true, images: [],
        model_override: null, effort_override: effort, permission_mode_override: null,
      },
    });
  }
  return { threadId, paneId };
}

const workspaces = async (ctx) => (await ctx.codemux("get_app_state", {})).workspaces;
const layoutOf = (ws) => JSON.stringify({ tabs: ws.tabs, surfaces: ws.surfaces, title: ws.title });

export async function run(ctx) {
  const rootId = ctx.env.WORKSPACE_ID;
  // The main checkout already holds an unrelated chat.
  await startChat(ctx, { workspaceId: rootId, cwd: ctx.env.REPO, provider: "claude", model: CLAUDE_MODEL });
  const created = await ctx.codemux("create_worktree_workspace", {
    repo_path: ctx.env.REPO, branch: "optimize-ci-threads", new_branch: true, base: "main",
  });
  const coordinatorId = created?.data?.workspace_id ?? created?.workspace_id;
  const coordinator = (await workspaces(ctx)).find((w) => w.workspace_id === coordinatorId);
  ctx.check("coordinator is a linked worktree workspace", !!coordinator?.worktree_path, coordinator && { cwd: coordinator.cwd, worktree_path: coordinator.worktree_path });
  const root = (await workspaces(ctx)).find((w) => w.workspace_id === rootId);
  ctx.observe("main checkout", { id: rootId, cwd: root.cwd, protected: root.protected, tabs: root.tabs.length });
  ctx.observe("coordinator workspace", { id: coordinatorId, cwd: coordinator.cwd });
  const rootBefore = layoutOf(root);
  const countBefore = (await workspaces(ctx)).length;

  const prompt = [
    "Use the codemux orchestration tools exactly as follows, without asking questions.",
    `1. Call thread_launch with no workspace_id and these arguments: {"provider":"codex","model":"${CODEX_MODEL}","effort":"low","title":"${TITLES.codex}","message":"Reply with the single word READY.","client_request_id":"live-codex-1"}.`,
    `2. Call thread_launch with no workspace_id and these arguments: {"provider":"claude","model":"${CLAUDE_MODEL}","title":"${TITLES.claude}","message":"Reply with the single word READY.","client_request_id":"live-claude-1"}.`,
    `3. Call delegate_task with {"provider":"claude","model":"${CLAUDE_MODEL}","title":"${TITLES.delegated}","task":"List the files in the current directory and reply with how many there are.","client_request_id":"live-delegate-1"}.`,
    "4. Then reply with the single word LAUNCHED and end your turn.",
  ].join("\n");
  const coord = await startChat(ctx, {
    workspaceId: coordinatorId, cwd: coordinator.cwd, provider: "claude", model: CLAUDE_MODEL,
    permissionMode: "bypassPermissions", text: prompt,
  });
  ctx.observe("coordinator thread", coord.threadId);

  const children = await ctx.waitFor(async () => {
    const all = await workspaces(ctx);
    const mine = all.filter((w) => w.parent_workspace_id === coordinatorId);
    return mine.length >= 3 && mine.every((w) => w.tabs.length >= 1) ? mine : null;
  }, { timeout: 420_000, interval: 3_000, label: "three child workspaces" });

  for (const title of Object.values(TITLES)) {
    const child = children.find((w) => w.title === title);
    ctx.check(`child "${title}": own workspace nested under the coordinator`, !!child, children.map((w) => w.title));
    ctx.check(`child "${title}": coordinator's folder, no worktree, same project`,
      child.cwd === coordinator.cwd && child.worktree_path == null && child.project_root === coordinator.project_root,
      { cwd: child.cwd, worktree_path: child.worktree_path, project_root: child.project_root });
    ctx.observe(`child ${title}`, { id: child.workspace_id, tabs: child.tabs.map((t) => t.title) });
  }
  const launched = children.filter((w) => w.title !== TITLES.delegated);
  ctx.check("launched chats' tabs carry their titles", launched.every((w) => w.tabs[0].title === w.title), launched.map((w) => w.tabs.map((t) => t.title)));

  const sessions = await ctx.assert.sql(
    `SELECT thread_id, provider, workspace_id, title FROM agent_chat_sessions WHERE workspace_id IN (${children.map((w) => `'${w.workspace_id}'`).join(",")})`,
  );
  ctx.observe("child sessions", sessions);
  ctx.check("both providers launched", ["codex", "claude"].every((p) => sessions.some((s) => s.provider === p)), sessions);

  const after = await workspaces(ctx);
  ctx.check("main checkout layout unchanged", layoutOf(after.find((w) => w.workspace_id === rootId)) === rootBefore);
  ctx.check("exactly three new workspaces", after.length === countBefore + 3, { before: countBefore, after: after.length });
  const app = await ctx.codemux("get_app_state", {});
  ctx.check("focus never moved to a child", !children.some((w) => w.workspace_id === app.active_workspace_id), app.active_workspace_id);

  // Projects view: children indented under the coordinator.
  await ctx.driver.clickSelector('[aria-label="Projects view"]').catch(() => {});
  await ctx.shot("child-workspaces-projects");
  await ctx.driver.clickSelector('[aria-label="Activity view"]').catch(() => {});
  await ctx.shot("child-workspaces-inbox");
}
