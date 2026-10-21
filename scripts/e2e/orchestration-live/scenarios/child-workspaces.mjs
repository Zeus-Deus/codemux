// Child workspaces: a coordinator chat in a linked worktree starts two chats
// with thread_launch (no workspace) and delegates one task. Each child must
// become its own workspace, nested under the coordinator, in its folder and
// titled with the task, while the populated, protected main checkout stays
// as it was. Then the coordinator's checkout cannot be deleted while the
// launched chats use it, and closing the coordinator closes the delegated
// chat's workspace but not the launched ones. Last, with the checkout
// reopened as its own workspace and both launched agents busy in it, their
// workspaces close first and the checkout is deleted at once: the delete
// must wait for those agents to stop (or refuse), never remove the files
// under them.
//
// Run it on a restarted root (up.sh, down.sh, up.sh <root>): the seeded
// checkout is stamped protected by the boot-time backfill.
import { randomUUID } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";

export const name = "child-workspaces";
export const description =
  "A worktree coordinator calls thread_launch twice without a workspace and delegate_task once; " +
  "assert three titled child workspaces nested under it and an unchanged protected main checkout; " +
  "deleting the coordinator's checkout refuses, and closing it closes only the delegated child; " +
  "deleting the checkout right after busy launched chats closed waits for their agents.";

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

const BRANCH = "optimize-ci-threads";
// A unique duration, so only these agents' commands match.
const SLEEP = "time.sleep(240.0137)";

/** Pids of this scenario's long-running agent commands, from /proc. */
function sleepers() {
  return readdirSync("/proc").filter((pid) => {
    if (!/^\d+$/.test(pid)) return false;
    try {
      const args = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
      return /python/.test(args[0]) && args.some((arg) => arg.includes(SLEEP));
    } catch {
      return false;
    }
  });
}

const workspaces = async (ctx) => (await ctx.codemux("get_app_state", {})).workspaces;
const layoutOf = (ws) => JSON.stringify({ tabs: ws.tabs, surfaces: ws.surfaces, title: ws.title });

export async function run(ctx) {
  const rootId = ctx.env.WORKSPACE_ID;
  // The main checkout already holds an unrelated chat.
  await startChat(ctx, { workspaceId: rootId, cwd: ctx.env.REPO, provider: "claude", model: CLAUDE_MODEL });
  const created = await ctx.codemux("create_worktree_workspace", {
    repo_path: ctx.env.REPO, branch: BRANCH, new_branch: true, base: "main",
  });
  const coordinatorId = created?.data?.workspace_id ?? created?.workspace_id;
  const coordinator = (await workspaces(ctx)).find((w) => w.workspace_id === coordinatorId);
  ctx.check("coordinator is a linked worktree workspace", !!coordinator?.worktree_path, coordinator && { cwd: coordinator.cwd, worktree_path: coordinator.worktree_path });
  const root = (await workspaces(ctx)).find((w) => w.workspace_id === rootId);
  ctx.observe("main checkout", { id: rootId, cwd: root.cwd, protected: root.protected, tabs: root.tabs.length });
  ctx.check("main checkout is protected", root.protected === true, { protected: root.protected });
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
    // Opening it must show its chat, not the empty-workspace screen.
    const tab = child.tabs.find((t) => t.tab_id === child.active_tab_id);
    ctx.check(`child "${title}": its chat is selected inside it`,
      !!tab && tab.surface_id === child.active_surface_id && child.surfaces.some((s) => s.surface_id === child.active_surface_id),
      { active_tab_id: child.active_tab_id, active_surface_id: child.active_surface_id });
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

  // The launched chats still work in the coordinator's checkout: deleting
  // its files refuses and changes nothing. Through the control socket (the
  // MCP workspace_close path): the web remote never deletes files.
  const family = async () => (await workspaces(ctx))
    .filter((w) => w.workspace_id === coordinatorId || w.parent_workspace_id === coordinatorId).length;
  const deleteError = await ctx.codemux("close_workspace", {
    workspace_id: coordinatorId, delete_worktree: true, force_delete: true,
  }).then(() => null, (error) => error.message);
  ctx.observe("refused delete", deleteError);
  ctx.check("deleting the coordinator's checkout refuses while launched chats use it",
    /2 workspaces still use this checkout/.test(deleteError ?? ""), deleteError);
  ctx.check("the refused delete closed nothing", (await family()) === 4);
  // The sandbox's HOME is <root>/home on this side.
  const onHost = coordinator.worktree_path.replace(process.env.HOME, `${ctx.env.ROOT}/home`);
  ctx.check("the coordinator's files stay", existsSync(onHost), onHost);

  // Closing the coordinator closes the chat it delegated, as its tab used
  // to close with it; chats it launched are independent and stay.
  const delegatedId = children.find((w) => w.title === TITLES.delegated).workspace_id;
  await ctx.driver.invoke("close_workspace", { workspaceId: coordinatorId, forceDelete: false });
  const remaining = await ctx.waitFor(async () => {
    const all = await workspaces(ctx);
    return all.some((w) => w.workspace_id === coordinatorId || w.workspace_id === delegatedId) ? null : all;
  }, { timeout: 60_000, interval: 1_000, label: "coordinator and delegated child to close" });
  ctx.check("the coordinator and its delegated chat's workspace closed", true);
  ctx.check("the launched chats' workspaces stay",
    launched.every((w) => remaining.some((r) => r.workspace_id === w.workspace_id)),
    remaining.map((w) => w.title));
  ctx.check("main checkout layout still unchanged",
    layoutOf(remaining.find((w) => w.workspace_id === rootId)) === rootBefore);
  await ctx.driver.clickSelector('[aria-label="Projects view"]').catch(() => {});
  await ctx.shot("child-workspaces-after-parent-close");

  // A chat child workspace closed first can still be stopping its agent
  // when the checkout's own workspace deletes the files.
  const launchedSessions = sessions.filter((s) => launched.some((w) => w.workspace_id === s.workspace_id));
  // From the store: conversation_read only reaches the harness workspace.
  for (const s of launchedSessions) {
    await ctx.waitFor(async () => (await ctx.assert.sql(
      `SELECT 1 FROM agent_chat_search WHERE thread_id = '${s.thread_id}' AND role = 'assistant' AND content LIKE '%READY%'`,
    )).length > 0 || null, { timeout: 300_000, label: `${s.provider} launched chat replied` });
  }
  const reopened = await ctx.codemux("create_worktree_workspace", {
    repo_path: ctx.env.REPO, branch: BRANCH, new_branch: false,
  });
  const ownerId = reopened?.data?.workspace_id ?? reopened?.workspace_id;
  const owner = (await workspaces(ctx)).find((w) => w.workspace_id === ownerId);
  ctx.check("the checkout reopened as its own workspace",
    owner?.worktree_path === coordinator.worktree_path && owner.parent_workspace_id == null,
    owner && { id: ownerId, worktree_path: owner.worktree_path, parent: owner.parent_workspace_id });
  const busy = `Run this exact shell command with your shell tool and wait for it to finish: python3 -c 'import time; ${SLEEP}'. Then reply DONE.`;
  for (const s of launchedSessions) {
    await ctx.driver.invoke("agent_chat_send_turn", {
      provider: s.provider,
      input: {
        thread_id: s.thread_id, text: busy, display_text: busy, skill_ids: [],
        skill_text: null, include_plugins: true, images: [], model_override: null,
        effort_override: s.provider === "codex" ? "low" : null, permission_mode_override: null,
      },
    });
  }
  await ctx.waitFor(() => (sleepers().length >= launchedSessions.length ? true : null),
    { timeout: 240_000, interval: 2_000, label: "both launched agents running their command" });
  const busyPids = sleepers();

  const closing = Date.now();
  for (const w of launched) {
    await ctx.driver.invoke("close_workspace", { workspaceId: w.workspace_id, forceDelete: false });
  }
  const closedMs = Date.now() - closing;
  const runningAtDelete = sleepers();
  const deleting = Date.now();
  const deleted = await ctx.codemux("close_workspace", {
    workspace_id: ownerId, delete_worktree: true, force_delete: true,
  }).then(() => ({ ok: true }), (error) => ({ ok: false, error: error.message }));
  const deleteMs = Date.now() - deleting;
  const left = sleepers();
  ctx.observe("closed-first delete", {
    busyPids, closedMs, runningAtDelete, deleteMs, deleted, commandsLeftAfterDelete: left,
  });
  const gone = (await workspaces(ctx))
    .filter((w) => [ownerId, ...launched.map((l) => l.workspace_id)].includes(w.workspace_id));
  ctx.check("the closed chats' and the checkout's workspaces are gone", gone.length === 0, gone.map((w) => w.title));
  if (deleted.ok) {
    ctx.check("the delete removed the files", !existsSync(onHost), { deleteMs });
    ctx.check("no closed chat's command still runs in the deleted checkout", left.length === 0, left);
  } else {
    ctx.check("the delete refused for an agent still stopping and kept the files",
      /still stopping|did not confirm|did not stop/.test(deleted.error) && existsSync(onHost), deleted);
  }
  await ctx.driver.clickSelector('[aria-label="Projects view"]').catch(() => {});
  await ctx.shot("child-workspaces-after-closed-first-delete");
}
