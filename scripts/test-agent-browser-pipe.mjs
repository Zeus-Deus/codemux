// Native smoke gate for the managed Chromium debugging transport.
// Every browser profile and daemon socket lives under one disposable directory.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, readFile, readdir, readlink, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { execFile as execFileCallback } from 'node:child_process';

const execFile = promisify(execFileCallback);
assert(process.argv[2], 'Pass the patched sidecar binary path');
const binary = resolve(process.argv[2]);
const root = await mkdtemp(join(process.platform === 'linux' ? '/var/tmp' : process.platform === 'darwin' ? '/tmp' : tmpdir(), 'cmx-p-'));
const socketDir = join(root, 's');
const managedProfile = join(root, 'managed');
const blankProfile = join(root, 'blank');
const failedProfile = join(root, 'failed');
const externalProfile = join(root, 'external');
const sessionPrefix = `p${randomUUID().slice(0, 8)}`;
const env = {
  ...process.env,
  AGENT_BROWSER_SOCKET_DIR: socketDir,
  CODEMUX_BROWSER_SECRET: randomBytes(32).toString('hex'),
};
const sessions = [];
let externalChrome;
let primaryError;

async function run(file, args, options = {}) {
  const started = performance.now();
  try {
    return { ...await execFile(file, args, { env, timeout: 20_000, maxBuffer: 4 << 20, ...options }),
      ok: true, elapsedMs: Math.round(performance.now() - started) };
  } catch (error) {
    return { stdout: error.stdout ?? '', stderr: error.stderr ?? '', ok: false, error,
      elapsedMs: Math.round(performance.now() - started) };
  }
}

function requireSuccess(result, description) {
  assert(result.ok, `${description} failed after ${result.elapsedMs} ms ` +
    `(code=${String(result.error?.code)}, signal=${String(result.error?.signal)}, ` +
    `killed=${String(result.error?.killed)}):\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}\n${result.error}`);
  return result.stdout;
}

async function waitFor(predicate, description, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`Timed out waiting for ${description}`);
}

function chromePath() {
  if (process.env.CODEMUX_CHROMIUM_PATH) {
    assert(existsSync(process.env.CODEMUX_CHROMIUM_PATH), 'CODEMUX_CHROMIUM_PATH is missing');
    return process.env.CODEMUX_CHROMIUM_PATH;
  }
  const candidates = process.platform === 'win32'
    ? [
        'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
        'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
        'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
      ]
    : process.platform === 'darwin'
      ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
          '/Applications/Chromium.app/Contents/MacOS/Chromium']
      : ['/usr/bin/google-chrome', '/usr/bin/google-chrome-stable', '/usr/bin/chromium'];
  const found = candidates.find(existsSync);
  assert(found, `No Chromium executable found; set CODEMUX_CHROMIUM_PATH`);
  return found;
}

async function cli(session, args, options) {
  return run(binary, ['--session', session, ...args], options);
}

async function closeManaged(session, description) {
  const deadline = Date.now() + 24_000;
  for (;;) {
    const remaining = deadline - Date.now();
    assert(remaining > 0, `${description} did not finish cleanup within 24 seconds`);
    const result = await cli(session, ['close'], { timeout: Math.min(20_000, remaining) });
    if (result.ok) return result.stdout;
    const output = `${result.stdout}\n${result.stderr}`.trim();
    if (!/^(?:✗\s*)?Managed Chrome cleanup is still in progress; retry the command$/.test(output)) {
      return requireSuccess(result, description);
    }
    await new Promise((done) => setTimeout(done, 200));
  }
}

async function unixChromePid(profile) {
  if (process.platform === 'linux') {
    for (const entry of await readdir('/proc')) {
      if (!/^\d+$/.test(entry)) continue;
      let args;
      try { args = (await readFile(`/proc/${entry}/cmdline`, 'utf8')).split('\0'); } catch { continue; }
      // Chromium rewrites argv into one NUL-terminated command-line string.
      const command = args.join(' ');
      if (command.includes(`--user-data-dir=${profile}`) &&
          command.includes('--remote-debugging-pipe') && !command.includes(' --type=')) {
        return Number(entry);
      }
    }
    return null;
  }
  const { stdout } = await execFile('ps', ['-axww', '-o', 'pid=,command=']);
  for (const line of stdout.split('\n')) {
    if (!line.includes(`--user-data-dir=${profile}`) ||
        !line.includes('--remote-debugging-pipe') || line.includes(' --type=')) continue;
    return Number(line.trim().match(/^\d+/)?.[0]) || null;
  }
  return null;
}

async function checkUnixPipesAndListeners(pid) {
  if (process.platform === 'linux') {
    const inodes = new Set();
    for (const fd of await readdir(`/proc/${pid}/fd`)) {
      const target = await readlink(`/proc/${pid}/fd/${fd}`).catch(() => '');
      const inode = target.match(/^socket:\[(\d+)\]$/)?.[1];
      if (inode) inodes.add(inode);
      if (fd === '3' || fd === '4') assert(inode, `Chromium fd ${fd} is not a pipe socket: ${target}`);
    }
    for (const fd of ['3', '4']) assert(existsSync(`/proc/${pid}/fd/${fd}`), `Chromium fd ${fd} is closed`);
    for (const table of ['/proc/net/tcp', '/proc/net/tcp6']) {
      const rows = (await readFile(table, 'utf8')).trim().split('\n').slice(1);
      for (const row of rows) {
        const fields = row.trim().split(/\s+/);
        assert(!(fields[3] === '0A' && inodes.has(fields[9])), `Chromium owns TCP listener ${row}`);
      }
    }
  } else {
    const descriptors = await run('lsof', ['-nP', '-a', '-p', String(pid), '-d', '3,4', '-F', 'ft']);
    requireSuccess(descriptors, 'inspect Chromium pipe descriptors');
    const types = new Map();
    let currentFd;
    for (const line of descriptors.stdout.split('\n')) {
      if (/^f[34]/.test(line)) currentFd = line[1];
      if (currentFd && line.startsWith('t')) types.set(currentFd, line.slice(1));
    }
    for (const fd of ['3', '4']) {
      assert.match(types.get(fd) ?? '', /^(unix|PIPE|FIFO)$/i, `Chromium fd ${fd} is not a pipe`);
    }
    const listeners = await run('lsof', ['-nP', '-a', '-p', String(pid), '-iTCP', '-sTCP:LISTEN']);
    assert(listeners.ok || listeners.error?.code === 1,
      `Could not inspect Chromium TCP listeners: ${listeners.error}`);
    assert(!listeners.stdout.trim() || listeners.stdout.trim().split('\n').length <= 1,
      `Chromium owns a TCP listener:\n${listeners.stdout}`);
  }
}

async function checkWindowsHandlesAndListeners(profile) {
  const script = resolve('scripts/test-agent-browser-pipe-windows.ps1');
  const result = await run('pwsh', ['-NoProfile', '-File', script, '-ProfilePath', profile],
    { timeout: 45_000 });
  requireSuccess(result, 'inspect Chromium inherited handles and listeners');
  const pid = Number(result.stdout.trim());
  assert(Number.isInteger(pid) && pid > 0, `Invalid Chromium PID: ${result.stdout}`);
  return pid;
}

function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { if (error.code === 'ESRCH') return false; throw error; }
}

async function stopExternal() {
  if (!externalChrome) return;
  if (process.platform === 'win32') {
    if (externalChrome.exitCode === null && externalChrome.signalCode === null) {
      await run('taskkill', ['/PID', String(externalChrome.pid), '/T', '/F'], { timeout: 5_000 });
    }
  } else {
    try { process.kill(-externalChrome.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
  }
  await waitFor(() => externalChrome.exitCode !== null || externalChrome.signalCode !== null,
    'external Chromium exit', 5_000);
}

try {
  await Promise.all([mkdir(socketDir), mkdir(managedProfile), mkdir(blankProfile),
    mkdir(failedProfile), mkdir(externalProfile)]);
  const chrome = chromePath();
  const managed = `${sessionPrefix}-managed`;
  sessions.push(managed);
  const dataUrl = `data:text/html,${encodeURIComponent('<title>Pipe smoke</title><h1>private transport</h1>')}`;
  requireSuccess(await cli(managed, ['--executable-path', chrome, '--profile', managedProfile,
    'open', dataUrl], { timeout: 35_000 }), 'open managed Chromium');
  assert.match(requireSuccess(await cli(managed, ['get', 'title']), 'round-trip CDP command'), /Pipe smoke/);
  assert(!existsSync(join(managedProfile, 'DevToolsActivePort')), 'Managed Chromium wrote DevToolsActivePort');
  const chromePid = process.platform === 'win32'
    ? await checkWindowsHandlesAndListeners(managedProfile)
    : await waitFor(() => unixChromePid(managedProfile), 'managed Chromium PID');
  if (process.platform !== 'win32') await checkUnixPipesAndListeners(chromePid);
  for (const [command, expected] of [
    [['get', 'cdp-url'], /managed browser CDP endpoints are unavailable/i],
    [['inspect'], /inspect is unavailable for managed browsers/i],
  ]) {
    const result = await cli(managed, command);
    assert(!result.ok && expected.test(`${result.stdout}\n${result.stderr}`),
      `${command.join(' ')} exposed managed CDP: ${result.stdout}\n${result.stderr}`);
  }
  await closeManaged(managed, 'close managed Chromium');
  sessions.pop();
  await waitFor(() => !alive(chromePid), 'managed Chromium exit');

  const blank = `${sessionPrefix}-blank`;
  sessions.push(blank);
  requireSuccess(await cli(blank, ['--executable-path', chrome, '--profile', blankProfile,
    'open'], { timeout: 35_000 }), 'launch managed Chromium without URL');
  assert.match(requireSuccess(await cli(blank, ['get', 'url']), 'no-URL CDP command'), /about:blank/);
  assert(!existsSync(join(blankProfile, 'DevToolsActivePort')),
    'No-URL managed launch wrote DevToolsActivePort');
  const blankPid = process.platform === 'win32'
    ? await checkWindowsHandlesAndListeners(blankProfile) : await unixChromePid(blankProfile);
  assert(blankPid, 'No-URL Chromium process was not found');
  await closeManaged(blank, 'close no-URL Chromium');
  sessions.pop();
  await waitFor(() => !alive(blankPid), 'no-URL Chromium exit');

  for (const [index, flag] of ['--remote-debugging-port=0', '-remote-debugging-port=0',
    '/remote-debugging-port=0', '--remote-debugging-pipe',
    '--remote-debugging-io-pipes=1,2'].entries()) {
    const rejected = `${sessionPrefix}-reject-${index}`;
    sessions.push(rejected);
    const result = await cli(rejected, ['--executable-path', chrome,
      '--profile', failedProfile, '--args', flag, 'open', 'about:blank']);
    assert(!result.ok && /Custom remote debugging arguments are unavailable/.test(
      `${result.stdout}\n${result.stderr}`), `Managed Chrome accepted ${flag}: ${result.stdout}\n${result.stderr}`);
    requireSuccess(await cli(rejected, ['close']), `close rejected session ${index}`);
    sessions.pop();
  }
  assert(!existsSync(join(failedProfile, 'DevToolsActivePort')));

  const fake = join(root, process.platform === 'win32' ? 'fake-chrome.exe' : 'fake-chrome');
  requireSuccess(await run('rustc', ['--edition=2021', 'scripts/fixtures/browser-pipe-failure.rs',
    '-o', fake], { timeout: 30_000 }), 'build broken pipe fixture');
  const failed = `${sessionPrefix}-failed`;
  sessions.push(failed);
  const failedLaunch = await cli(failed, ['--executable-path', fake,
    '--profile', failedProfile, 'open', 'about:blank'], { timeout: 35_000 });
  assert(!failedLaunch.ok && /pipe/i.test(`${failedLaunch.stdout}\n${failedLaunch.stderr}`),
    `Broken pipe launch did not fail closed: ${failedLaunch.stdout}\n${failedLaunch.stderr}`);
  const fakePid = Number(await readFile(join(failedProfile, 'fake-chrome.pid'), 'utf8'));
  await waitFor(() => !alive(fakePid), 'broken pipe child cleanup');

  externalChrome = spawn(chrome, ['--headless=new', '--no-first-run', '--no-default-browser-check',
    '--disable-background-networking', '--remote-debugging-port=0',
    `--user-data-dir=${externalProfile}`, 'about:blank'], {
    detached: process.platform !== 'win32', stdio: ['ignore', 'ignore', 'pipe'], env,
  });
  externalChrome.stderr.on('data', () => {});
  const activePort = await waitFor(async () => {
    try { return (await readFile(join(externalProfile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; }
    catch { return null; }
  }, 'external Chromium debugging port');
  assert(/^\d+$/.test(activePort));
  const attached = `${sessionPrefix}-attached`;
  sessions.push(attached);
  requireSuccess(await cli(attached, ['--cdp', activePort, 'get', 'url']), 'attach to external CDP');
  assert.match(requireSuccess(await cli(attached, ['--cdp', activePort, 'get', 'cdp-url']),
    'read external CDP URL'), /ws:\/\//);
  console.log('Native browser pipe smoke passed: round trip, private handles, no managed TCP listener, denied overrides/endpoints, failure cleanup, and external CDP attachment.');
} catch (error) {
  primaryError = error;
  throw error;
} finally {
  for (const session of sessions.reverse()) await cli(session, ['close']).catch(() => {});
  let cleanupError;
  try { await stopExternal(); } catch (error) { cleanupError = error; }
  try {
    // Windows can release a closed Chromium profile handle a moment after the
    // CLI reports close. A persistent lock still fails this bounded cleanup.
    await rm(root, { recursive: true, force: true,
      maxRetries: process.platform === 'win32' ? 10 : 0, retryDelay: 200 });
  } catch (error) { cleanupError ??= error; }
  if (cleanupError) {
    if (primaryError) console.error('Fixture cleanup also failed:', cleanupError);
    else throw cleanupError;
  }
}
