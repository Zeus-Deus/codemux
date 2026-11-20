import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir, rename, rm, stat } from 'node:fs/promises';
import { join, resolve, dirname } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

assert.equal(process.platform, 'win32');
assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Requires disposable GitHub CI');
assert.equal(process.env.RUNNER_ENVIRONMENT, 'github-hosted');
const evidenceDir = resolve('windows-claude-evidence');
const install = JSON.parse((await readFile(join(evidenceDir, 'install.json'), 'utf8')).replace(/^\uFEFF/, ''));
const evidence = { commit: process.env.GITHUB_SHA, install, checks: [] };
const expectPathFailure = process.argv.includes('--expect-path-failure');
const owned = new Set();
function start(program, args, env = process.env, options = {}) {
  const child = spawn(program, args, { env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], ...options });
  owned.add(child);
  child.once('exit', () => owned.delete(child));
  child.once('error', () => owned.delete(child));
  let output = '';
  child.stdout.on('data', data => { output = (output + data).slice(-20000); });
  child.stderr.on('data', data => { output = (output + data).slice(-20000); });
  child.done = new Promise((done, fail) => {
    child.once('error', fail);
    child.once('exit', code => done({ code, output }));
  });
  child.done.catch(() => {});
  return child;
}
async function stop(child) {
  if (!owned.has(child)) return;
  // Only our own PID and descendants; never terminate by executable name.
  const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true });
  await new Promise(done => killer.once('exit', done));
  await child.done;
}
async function run(program, args, env, options) {
  const child = start(program, args, env, options);
  const timer = setTimeout(() => { void stop(child); }, 90000);
  try {
    const result = await child.done;
    assert.equal(result.code, 0, `${program}: ${result.output}`);
    return result.output;
  } finally { clearTimeout(timer); }
}
async function until(label, fn, timeout = 60000) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { const value = await fn(); if (value) return value; } catch (error) { last = error; }
    await delay(250);
  }
  throw Error(`${label}: ${last?.message ?? 'timed out'}`);
}
const exists = file => stat(file).then(() => true, error => {
  if (error.code === 'ENOENT') return false;
  throw error;
});
// Holds `file` open with no sharing, as a still-running old sidecar holds its
// .exe, until the returned function releases it.
async function holdOpen(file) {
  const holder = start('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command',
    "$f = [System.IO.File]::Open($env:CODEMUX_HELD_FILE, 'Open', 'Read', 'None'); [Console]::Out.WriteLine('held'); [void][Console]::In.ReadLine(); $f.Close()"],
  { ...process.env, CODEMUX_HELD_FILE: file });
  const timer = setTimeout(() => { void stop(holder); }, 60000);
  try {
    await new Promise((resolve, reject) => {
      createInterface({ input: holder.stdout }).on('line', line => { if (line.trim() === 'held') resolve(); });
      holder.done.then(result => reject(Error(`Holding ${file} failed: ${result.output}`)));
    });
  } finally { clearTimeout(timer); }
  return async () => {
    holder.stdin.end('\n');
    const result = await holder.done;
    assert.equal(result.code, 0, result.output);
  };
}
// Whether Windows will delete `file` at the next restart.
async function deletedAtRestart(file) {
  const { output } = await start('reg.exe', ['query', 'HKLM\\SYSTEM\\CurrentControlSet\\Control\\Session Manager', '/v', 'PendingFileRenameOperations']).done;
  return output.toLowerCase().includes(file.toLowerCase());
}
function rpc(program, env) {
  const child = start(program, [], env);
  const pending = new Map();
  let id = 0;
  createInterface({ input: child.stdout }).on('line', line => {
    const message = JSON.parse(line);
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    clearTimeout(request.timer);
    if (message.error) request.reject(Error(JSON.stringify(message.error)));
    else request.resolve(message.result);
  });
  child.done.then(result => {
    for (const request of pending.values()) { clearTimeout(request.timer); request.reject(Error(result.output)); }
    pending.clear();
  });
  return {
    child,
    request(method, params) {
      return new Promise((resolve, reject) => {
        const requestId = ++id;
        const timer = setTimeout(() => { pending.delete(requestId); reject(Error(`${method} timed out`)); }, 30000);
        pending.set(requestId, { resolve, reject, timer });
        child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: requestId, method, params }) + '\n');
      });
    },
  };
}
// Preserve the runner's toolchain, but deliberately remove every Claude install
// entry. This is the environment an already-running desktop keeps after install.
const pathKey = Object.keys(process.env).find(key => key.toLowerCase() === 'path');
assert.ok(pathKey);
const stalePath = process.env[pathKey].split(';').filter(entry => {
  const value = entry.replaceAll('\\', '/').toLowerCase();
  return !value.includes('/.local/bin') && !value.includes('anthropic.claudecode');
}).join(';');
const staleEnv = { ...process.env, [pathKey]: stalePath };
const repairedEnv = { ...staleEnv, [pathKey]: `${dirname(install.cli)};${stalePath}` };
// The bundled multi-role sidecar runs Claude when started without a role.
const sidecar = resolve('sidecar/dist/codemux-sidecar-x86_64-pc-windows-msvc.exe');
const stagedSidecar = resolve('src-tauri/binaries/codemux-sidecar-x86_64-pc-windows-msvc.exe');
let debugPolicy = false;
let socket;
let hiddenCli;
let hiddenSidecar;
try {
  for (const [label, env, expected] of [['stale', staleEnv, false], ['repaired', repairedEnv, true]]) {
    const client = rpc(sidecar, env);
    try {
      const probe = await client.request('probe-installed', {});
      assert.equal(probe.installed, expected);
      const absolute = await client.request('probe-installed', { binaryPath: install.cli });
      assert.equal(absolute.installed, true);
      assert.notEqual(absolute.version, 'unknown');
      const models = await client.request('list-models', { cwd: process.env.RUNNER_TEMP, pathToClaudeCodeExecutable: install.cli });
      assert.ok(models.models.length > 0, 'Real SDK initialization must return models without inference');
      if (label === 'repaired') {
        // Exercise the same bare executable name the agent GUI passes. Session
        // initialization exchanges control messages only; never submit a turn.
        await client.request('start-session', {
          threadId: 'windows-install-smoke', cwd: process.env.RUNNER_TEMP,
          pathToClaudeCodeExecutable: 'claude',
        });
        const initialized = await client.request('initialization-result', { threadId: 'windows-install-smoke' });
        assert.ok(initialized && typeof initialized === 'object');
        await client.request('stop-session', { threadId: 'windows-install-smoke' });
      }
      evidence.checks.push({ label, probe, absolute, modelCount: models.models.length });
      console.log(`Real sidecar (${label} PATH): ${JSON.stringify(evidence.checks.at(-1))}`);
    } finally { await stop(client.child); }
  }

  const installer = resolve(process.argv[2]);
  const appDir = join(process.env.RUNNER_TEMP, 'Codemux install with spaces');
  // Releases through v0.27.0 shipped these; src-tauri/windows/installer-hooks.nsh
  // removes them after an install and before an uninstall. Seed them as an
  // older install would have left them, and install the way the updater does.
  const retiredSidecars = ['codemux-claude-sidecar', 'codemux-managed-workflow-sidecar', 'codemux-plugin-host']
    .map(name => join(appDir, 'binaries', `${name}-x86_64-pc-windows-msvc.exe`));
  const seedRetiredSidecars = () => Promise.all(retiredSidecars.map(file => writeFile(file, 'retired')));
  const assertRetiredSidecarsGone = async when => {
    for (const file of retiredSidecars) {
      await assert.rejects(stat(file), { code: 'ENOENT' }, `${file} must not survive ${when}`);
    }
  };
  // The hook deletes them in order and retries a file that is in use, so the
  // last one is held open: a sidecar of the old app that is still exiting.
  const heldRetired = retiredSidecars.at(-1);
  const retiredOutcome = {};
  await mkdir(join(appDir, 'binaries'), { recursive: true });
  await seedRetiredSidecars();
  let release = await holdOpen(heldRetired);
  try {
    // NSIS requires /D= to be the final, unquoted argument, including spaces.
    const installing = start(installer, ['/S', '/UPDATE', `/D=${appDir}`], undefined, { windowsVerbatimArguments: true });
    const timer = setTimeout(() => { void stop(installing); }, 300000);
    let exited = false;
    installing.done.then(() => { exited = true; });
    try {
      // Once the others are gone the hook is retrying the held file.
      await until('Install hook reaches the held retired sidecar', async () => {
        if (exited) return true;
        for (const file of retiredSidecars.slice(0, -1)) if (await exists(file)) return false;
        return true;
      }, 300000);
      await release(); release = undefined;
      const result = await installing.done;
      assert.equal(result.code, 0, `${installer}: ${result.output}`);
    } finally { clearTimeout(timer); }
  } finally { await release?.(); }
  // Released within its retries, it is deleted. Released too late, it must
  // at least be scheduled for deletion at restart.
  if (await exists(heldRetired)) {
    assert.ok(await deletedAtRestart(heldRetired), `${heldRetired} survived the update install unscheduled`);
    retiredOutcome.install = 'deleted-at-restart';
    await rm(heldRetired);
  } else retiredOutcome.install = 'deleted-after-release';
  await assertRetiredSidecarsGone('an update install');
  const bundledSidecar = join(appDir, 'binaries', 'codemux-sidecar-x86_64-pc-windows-msvc.exe');
  assert.ok((await stat(bundledSidecar)).size > 0, 'Installer must ship the Claude sidecar');
  evidence.bundledSidecar = bundledSidecar;
  // A debug app can fall back to the checkout's staged sidecar. Hide that
  // copy so GUI checks prove the installed resource actually works.
  await rename(stagedSidecar, `${stagedSidecar}.smoke-backup`);
  hiddenSidecar = `${stagedSidecar}.smoke-backup`;
  await run('powershell.exe', ['-NoProfile', '-File', 'scripts/windows-webview-debug.ps1', '-Mode', 'enable']);
  debugPolicy = true;
  const guiCases = [['stale', staleEnv, !expectPathFailure], ['repaired', repairedEnv, true]];
  if (!expectPathFailure) guiCases.push(['installed-after-startup', staleEnv, false]);
  for (const [label, env, expected] of guiCases) {
    if (label === 'installed-after-startup') {
      hiddenCli = `${install.cli}.smoke-backup`;
      await rename(install.cli, hiddenCli);
    }
    const desktop = start(join(appDir, 'codemux.exe'), [], env);
    try {
      const target = await until('Installed app WebView2', async () => {
        const response = await fetch('http://127.0.0.1:9231/json/list', { signal: AbortSignal.timeout(1000) });
        return (await response.json()).find(page => page.type === 'page' && page.webSocketDebuggerUrl);
      });
      socket = new WebSocket(target.webSocketDebuggerUrl);
      await new Promise((done, fail) => { socket.addEventListener('open', done, { once: true }); socket.addEventListener('error', fail, { once: true }); });
      let nextId = 0;
      const pending = new Map();
      socket.addEventListener('message', event => {
        const message = JSON.parse(event.data);
        pending.get(message.id)?.(message);
        pending.delete(message.id);
      });
      const cdp = (method, params) => new Promise((resolve, reject) => {
        const id = ++nextId;
        const timer = setTimeout(() => { pending.delete(id); reject(Error(`${method} timed out`)); }, 30000);
        pending.set(id, result => { clearTimeout(timer); resolve(result); });
        socket.send(JSON.stringify({ id, method, params }));
      });
      await until('Native Tauri IPC initialized', async () => {
        const result = await cdp('Runtime.evaluate', {
          expression: 'typeof window.__TAURI_INTERNALS__?.invoke === "function"',
          returnByValue: true,
        });
        return result.result?.result?.value === true;
      });
      const result = await cdp('Runtime.evaluate', {
        expression: 'window.__TAURI_INTERNALS__.invoke("agent_chat_provider_health", {provider:"claude"})',
        awaitPromise: true, returnByValue: true,
      });
      assert.equal(result.result?.exceptionDetails, undefined, JSON.stringify(result));
      const health = result.result.result.value;
      evidence.checks.push({ label: `installed-gui-${label}`, health });
      assert.equal(health.installed, expected, JSON.stringify(health));
      assert.equal(health.version !== undefined && health.version !== null, expected);
      console.log(`Installed GUI (${label} PATH): ${JSON.stringify(health)}`);
      if (hiddenCli) {
        await rename(hiddenCli, install.cli);
        hiddenCli = undefined;
        const result = await cdp('Runtime.evaluate', {
          expression: 'window.__TAURI_INTERNALS__.invoke("agent_chat_provider_health", {provider:"claude"})',
          awaitPromise: true, returnByValue: true,
        });
        assert.equal(result.result?.exceptionDetails, undefined, JSON.stringify(result));
        const health = result.result.result.value;
        assert.equal(health.installed, true, 'Running GUI must find Claude installed after startup');
        assert.ok(health.version);
        evidence.checks.push({ label: 'same-running-gui-after-install', health });
        console.log(`Same running GUI after installation: ${JSON.stringify(health)}`);
      }
      socket.close(); socket = undefined;
    } finally {
      socket?.close(); socket = undefined;
      await stop(desktop);
      await until('WebView2 stopped', async () => {
        try { await fetch('http://127.0.0.1:9231/json/list', { signal: AbortSignal.timeout(500) }); return false; } catch { return true; }
      });
    }
  }

  // The uninstaller only knows the current resource list; the hook covers the
  // retired files. `_?=` runs it in place and waits for it to finish. This
  // time a held file stays in use throughout, so it must be scheduled for
  // deletion at restart. Scheduling needs administrator rights, which hosted
  // runners have; without them the hook can only log the file. A different
  // file than above, so a restart entry left by the install cannot count.
  await seedRetiredSidecars();
  const stillRunning = retiredSidecars[0];
  release = await holdOpen(stillRunning);
  try {
    await run(join(appDir, 'uninstall.exe'), ['/S', `_?=${appDir}`], undefined, { windowsVerbatimArguments: true });
  } finally { await release(); release = undefined; }
  for (const file of retiredSidecars.slice(1)) {
    await assert.rejects(stat(file), { code: 'ENOENT' }, `${file} must not survive an uninstall`);
  }
  const elevated = (await start('net.exe', ['session']).done).code === 0;
  if (elevated) assert.ok(await deletedAtRestart(stillRunning), `${stillRunning} survived the uninstall unscheduled`);
  else assert.ok(await exists(stillRunning));
  retiredOutcome.uninstall = elevated ? 'deleted-at-restart' : 'left-in-use-without-admin';
  await rm(stillRunning);
  await assert.rejects(stat(bundledSidecar), { code: 'ENOENT' }, 'Uninstall must remove the bundled sidecar');
  evidence.checks.push({ label: 'retired-sidecars-removed', ...retiredOutcome });
} catch (error) {
  evidence.failure = String(error.stack ?? error);
  throw error;
} finally {
  for (const child of owned) await stop(child);
  if (hiddenCli) await rename(hiddenCli, install.cli);
  if (hiddenSidecar) await rename(hiddenSidecar, stagedSidecar);
  if (debugPolicy) await run('powershell.exe', ['-NoProfile', '-File', 'scripts/windows-webview-debug.ps1', '-Mode', 'disable']);
  await writeFile(join(evidenceDir, 'smoke.json'), JSON.stringify(evidence, null, 2));
}
