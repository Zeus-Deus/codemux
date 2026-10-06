import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { join, resolve, dirname } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

assert.equal(process.platform, 'win32');
assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Requires disposable GitHub CI');
assert.equal(process.env.RUNNER_ENVIRONMENT, 'github-hosted');
const evidenceDir = resolve('windows-claude-evidence');
const install = JSON.parse((await readFile(join(evidenceDir, 'install.json'), 'utf8')).replace(/^\uFEFF/, ''));
const evidence = { commit: process.env.GITHUB_SHA, install, checks: [] };
const owned = new Set();
function start(program, args, env = process.env) {
  const child = spawn(program, args, { env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  owned.add(child);
  child.once('exit', () => owned.delete(child));
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
async function run(program, args, env) {
  const child = start(program, args, env);
  const timer = setTimeout(() => { void stop(child); }, 90000);
  try {
    const result = await child.done;
    assert.equal(result.code, 0, `${program}: ${result.output}`);
    return result.output;
  } finally { clearTimeout(timer); }
}
async function until(label, fn) {
  const deadline = Date.now() + 60000;
  let last;
  while (Date.now() < deadline) {
    try { const value = await fn(); if (value) return value; } catch (error) { last = error; }
    await delay(250);
  }
  throw Error(`${label}: ${last?.message ?? 'timed out'}`);
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
const sidecar = resolve('sidecar/claude-agent/dist/codemux-claude-sidecar-windows-x64.exe');
let debugPolicy = false;
let socket;
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
      evidence.checks.push({ label, probe, absolute, modelCount: models.models.length });
    } finally { await stop(client.child); }
  }

  const installer = resolve('windows-claude-input/codemux_0.23.1_x64-setup.exe');
  const appDir = join(process.env.RUNNER_TEMP, 'Codemux install with spaces');
  await mkdir(appDir, { recursive: true });
  await run(installer, ['/S', `/D=${appDir}`]);
  await run('powershell.exe', ['-NoProfile', '-File', 'scripts/addons/windows-webview-debug.ps1', '-Mode', 'enable']);
  debugPolicy = true;
  for (const [label, env, expected] of [['stale', staleEnv, false], ['repaired', repairedEnv, true]]) {
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
      const result = await cdp('Runtime.evaluate', {
        expression: 'window.__TAURI_INTERNALS__.invoke("agent_chat_provider_health", {provider:"claude"})',
        awaitPromise: true, returnByValue: true,
      });
      assert.equal(result.result?.exceptionDetails, undefined, JSON.stringify(result));
      const health = result.result.result.value;
      evidence.checks.push({ label: `installed-gui-${label}`, health });
      assert.equal(health.installed, expected, JSON.stringify(health));
      assert.equal(health.version !== undefined && health.version !== null, expected);
      console.log(`Stock installed GUI (${label} PATH): ${JSON.stringify(health)}`);
      socket.close(); socket = undefined;
    } finally {
      socket?.close(); socket = undefined;
      await stop(desktop);
      await until('WebView2 stopped', async () => {
        try { await fetch('http://127.0.0.1:9231/json/list', { signal: AbortSignal.timeout(500) }); return false; } catch { return true; }
      });
    }
  }
} catch (error) {
  evidence.failure = String(error.stack ?? error);
  throw error;
} finally {
  for (const child of owned) await stop(child);
  if (debugPolicy) await run('powershell.exe', ['-NoProfile', '-File', 'scripts/addons/windows-webview-debug.ps1', '-Mode', 'disable']);
  await writeFile(join(evidenceDir, 'smoke.json'), JSON.stringify(evidence, null, 2));
}
