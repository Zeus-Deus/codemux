// A paired browser (the path a phone takes) may remove a clean worktree, but
// never force a delete or delete a branch. The web-remote guard refuses those
// flags per command with its own text before the backend runs; the backend
// still refuses a dirty worktree; and the paired browser's delete dialog
// offers neither a branch checkbox nor "Force delete". No provider is used.
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { promisify } from "node:util";

const git = promisify(execFile);

export const name = "remote-delete-guard";
export const description =
  "Over the paired browser: removing a clean worktree works and keeps its branch; deleteBranch and forceDelete are refused " +
  "with the guard's text; a dirty worktree is refused and kept; the delete dialog offers no branch checkbox or force delete.";

const BRANCH_REFUSED = "web-remote: deleting a branch is only available in the Codemux desktop app";
const FORCE_REFUSED = "web-remote: force delete is only available in the Codemux desktop app";

/** A sandbox path under HOME, as the host sees it (<root>/home). */
const onHost = (ctx, path) => path.replace(process.env.HOME, `${ctx.env.ROOT}/home`);

/** The error text of a refused invoke, or null when it succeeded. */
const refusal = (promise) => promise.then(() => null, (error) => error.message);

async function worktree(ctx, repo, branch) {
  const created = await ctx.driver.invoke("create_worktree_workspace", {
    repoPath: repo, branch, newBranch: true, base: null, layout: "single", initialPrompt: null,
    agentPresetId: null, modelSelection: null, prNumber: null, select: false, initialChat: null,
  });
  const workspaceId = created?.workspace_id ?? created?.workspaceId;
  const state = await ctx.codemux("get_app_state", {});
  const workspace = state.workspaces.find((w) => w.workspace_id === workspaceId);
  ctx.check(`${branch}: created over the paired browser`, !!workspace?.worktree_path, workspace && { id: workspaceId, path: workspace.worktree_path });
  return { workspaceId, path: workspace.worktree_path };
}

/** Opens the dirty row's "Delete worktree…" dialog the way a right-click does. */
async function openDeleteDialog(ctx, card) {
  await ctx.driver.eval(`(() => {
    const el = document.querySelector(${JSON.stringify(card)});
    el.scrollIntoView({ block: "center" });
    const box = el.getBoundingClientRect();
    el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2,
      clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 }));
    return true;
  })()`);
  const item = `[...document.querySelectorAll('[role="menuitem"]')].find((el) => el.innerText.includes("Delete worktree"))`;
  await ctx.waitFor(`!!${item}`, { timeout: 10_000, label: "the row's Delete worktree item" });
  await ctx.driver.eval(`(${item}).click(), true`);
  await ctx.waitFor(`!!document.querySelector('[role="dialog"]') && document.querySelector('[role="dialog"]').innerText.includes("Delete worktree")`,
    { timeout: 10_000, label: "the delete dialog" });
}

export async function run(ctx) {
  const { env } = ctx;
  const tag = randomUUID().slice(0, 8);
  const repo = join(env.ROOT, `guard-repo-${tag}`);
  mkdirSync(repo, { recursive: true });
  const id = ["-c", "user.name=Orchestration Live Test", "-c", "user.email=orch-live@example.invalid"];
  await git("git", ["-C", repo, "init", "-q", "-b", "main"]);
  writeFileSync(join(repo, "README.md"), "# remote delete guard\n");
  await git("git", ["-C", repo, "add", "README.md"]);
  await git("git", ["-C", repo, ...id, "commit", "-q", "-m", "seed"]);

  const cleanBranch = `guard-clean-${tag}`;
  const dirtyBranch = `guard-dirty-${tag}`;
  const clean = await worktree(ctx, repo, cleanBranch);
  const dirty = await worktree(ctx, repo, dirtyBranch);
  try {
    // An uncommitted file, written the way a paired client edits files.
    await ctx.driver.invoke("write_file", { path: join(dirty.path, "notes.txt"), content: "not committed\n" });
    const close = (target, flags) => refusal(ctx.driver.invoke("close_workspace_with_worktree",
      { workspaceId: target.workspaceId, removeWorktree: true, deleteBranch: false, forceDelete: false, ...flags }));

    const branchRefused = await close(dirty, { deleteBranch: true });
    ctx.check("deleteBranch is refused with the guard's text", branchRefused?.endsWith(BRANCH_REFUSED), branchRefused);
    const forceRefused = await close(dirty, { forceDelete: true });
    ctx.check("forceDelete is refused with the guard's text", forceRefused?.endsWith(FORCE_REFUSED), forceRefused);
    ctx.check("the refusals left the dirty checkout in place", existsSync(onHost(ctx, dirty.path)));

    const removed = await close(clean, {});
    const { stdout: branches } = await git("git", ["-C", repo, "branch", "--list", cleanBranch]);
    ctx.check("a clean worktree is removed over the paired browser", removed === null && !existsSync(onHost(ctx, clean.path)), removed);
    ctx.check("its branch is kept", branches.trim().endsWith(cleanBranch), branches);
    const dirtyRefused = await close(dirty, {});
    ctx.check("the backend refuses a dirty worktree and keeps it",
      /uncommitted change\(s\)/.test(dirtyRefused ?? "") && existsSync(onHost(ctx, dirty.path)), dirtyRefused);

    // The paired browser's own dialog for the dirty row.
    await ctx.driver.clickSelector('[aria-label="Activity view"]').catch(() => {});
    const card = `[data-inbox-card="${dirty.workspaceId}"]`;
    const listed = await ctx.waitFor(`!!document.querySelector('${card}')`, { timeout: 30_000, label: "the dirty worktree's card" }).catch(() => false);
    if (!listed) {
      ctx.observe("dialog", "the dirty worktree has no card in the Activity view; dialog checks skipped");
      return;
    }
    await openDeleteDialog(ctx, card);
    const dialog = await ctx.driver.eval(`(() => { const d = document.querySelector('[role="dialog"]');
      return { text: d.innerText, checkboxes: d.querySelectorAll('[role="checkbox"], input[type="checkbox"]').length }; })()`);
    ctx.check("the paired browser's delete dialog keeps the branch and has no branch checkbox",
      dialog.checkboxes === 0 && dialog.text.includes("The branch is kept."), dialog);
    // Only the desktop halves of these shots are evidence. The phone shell
    // has no "Delete worktree…" (its workspace actions are rename, pin, mute
    // and archive), so at 390×844 the shot shows its workspace list.
    ctx.observe("phone shots", "remote-delete-dialog-mobile and remote-delete-refused-mobile show the phone shell's list, not the dialog or toast");
    await ctx.shot("remote-delete-dialog");
    // The phone-width shot can unmount the dialog; reopen it at desktop width.
    if (!(await ctx.driver.eval(`!!document.querySelector('[role="dialog"]')`))) await openDeleteDialog(ctx, card);
    await ctx.driver.eval(`[...document.querySelectorAll('[role="dialog"] button')].find((b) => b.innerText.trim() === "Delete").click(), true`);
    const toast = await ctx.waitFor(`(() => { const t = [...document.querySelectorAll('[data-sonner-toast]')]
      .find((el) => el.innerText.includes("Delete failed")); return t ? t.innerText : null; })()`,
    { timeout: 30_000, label: "the refusal toast" });
    const forceOffered = await ctx.driver.eval(`[...document.querySelectorAll('button')].some((b) => b.innerText.includes("Force delete"))`);
    ctx.check("the refusal says why and offers no force delete", /uncommitted/.test(toast) && !forceOffered, { toast, forceOffered });
    ctx.check("the dirty checkout is still there", existsSync(onHost(ctx, dirty.path)));
    await ctx.shot("remote-delete-refused");
  } finally {
    // The owner, through the control socket, may force it.
    for (const target of [clean, dirty]) {
      const open = (await ctx.codemux("get_app_state", {})).workspaces.some((w) => w.workspace_id === target.workspaceId);
      if (open) await ctx.codemux("close_workspace", { workspace_id: target.workspaceId, delete_worktree: true, force_delete: true }).catch(() => {});
    }
  }
}
